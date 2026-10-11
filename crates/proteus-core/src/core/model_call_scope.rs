use std::future::Future;

use crate::contracts::{CancellationToken, ModelCallOrigin, OperationOwner};

tokio::task_local! {
    static MODEL_CALL_ORIGIN: ModelCallOrigin;
}

pub(crate) fn current_model_call_origin() -> ModelCallOrigin {
    // Generic detached Model users have no host callback to scope explicitly.
    MODEL_CALL_ORIGIN
        .try_with(|origin| *origin)
        .unwrap_or(ModelCallOrigin::Direct)
}

pub(crate) async fn with_model_call_origin<F>(origin: ModelCallOrigin, future: F) -> F::Output
where
    F: Future,
{
    MODEL_CALL_ORIGIN.scope(origin, future).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn nested_and_concurrent_scopes_restore_and_isolate_origin() {
        assert_eq!(current_model_call_origin(), ModelCallOrigin::Direct);

        let direct = with_model_call_origin(ModelCallOrigin::Direct, async {
            tokio::task::yield_now().await;
            current_model_call_origin()
        });
        let compactor = with_model_call_origin(ModelCallOrigin::Auxiliary, async {
            assert_eq!(current_model_call_origin(), ModelCallOrigin::Auxiliary);
            let nested = with_model_call_origin(ModelCallOrigin::Direct, async {
                tokio::task::yield_now().await;
                current_model_call_origin()
            })
            .await;
            (nested, current_model_call_origin())
        });

        let (direct, (nested, restored)) = tokio::join!(direct, compactor);
        assert_eq!(direct, ModelCallOrigin::Direct);
        assert_eq!(nested, ModelCallOrigin::Direct);
        assert_eq!(restored, ModelCallOrigin::Auxiliary);
        assert_eq!(current_model_call_origin(), ModelCallOrigin::Direct);
    }
}

tokio::task_local! {
    static OPERATION_OWNER: OperationOwner;
    static TOOL_LINEAGE: Vec<usize>;
    static OPERATION_CANCELLATION: CancellationToken;
}

pub(crate) fn current_operation_owner() -> OperationOwner {
    OPERATION_OWNER
        .try_with(Clone::clone)
        .unwrap_or(OperationOwner::Workflow)
}
pub(crate) fn current_tool_lineage() -> Vec<usize> {
    TOOL_LINEAGE.try_with(Clone::clone).unwrap_or_default()
}
pub(crate) fn operation_cancellation(fallback: &CancellationToken) -> CancellationToken {
    OPERATION_CANCELLATION
        .try_with(Clone::clone)
        .unwrap_or_else(|_| fallback.clone())
}
pub(crate) async fn with_operation_scope<F: Future>(
    owner: OperationOwner,
    lineage: Vec<usize>,
    cancellation: CancellationToken,
    future: F,
) -> F::Output {
    OPERATION_OWNER
        .scope(
            owner,
            TOOL_LINEAGE.scope(lineage, OPERATION_CANCELLATION.scope(cancellation, future)),
        )
        .await
}
