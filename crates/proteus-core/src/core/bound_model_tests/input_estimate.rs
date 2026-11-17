use super::*;
use crate::model_standard::{
    ModelInputEstimate, ModelInputEstimateRequest, ModelPartTokenEstimate,
};

struct Estimator {
    invalid: bool,
    pending: bool,
}
#[async_trait]
impl Model for Estimator {
    fn id(&self) -> std::borrow::Cow<'static, str> {
        "selected".into()
    }
    fn capabilities(&self, _: &ModelRef) -> Result<ModelCapabilities> {
        Ok(ModelCapabilities::empty())
    }
    async fn stream(&self, _: CanonicalModelRequest) -> Result<ModelEventStream> {
        panic!("sizing must not perform inference")
    }
    async fn estimate_input(
        &self,
        request: ModelInputEstimateRequest,
    ) -> Result<Option<ModelInputEstimate>> {
        assert_eq!(request.model, ModelRef::new("selected", "model"));
        if self.pending {
            return futures_util::future::pending().await;
        }
        Ok(Some(ModelInputEstimate {
            parts: vec![ModelPartTokenEstimate {
                part_id: if self.invalid {
                    uuid::Uuid::new_v4()
                } else {
                    request.parts[0].part_id
                },
                tokens: 0,
            }],
        }))
    }
}
struct NoEstimateHooks;
#[async_trait]
impl crate::contracts::ExecutionHooks for NoEstimateHooks {
    async fn apply(&self, _: crate::contracts::HookInput) -> Result<crate::contracts::HookEvent> {
        panic!("sizing must not enter BeforeModel")
    }
}
fn estimate_request() -> ModelInputEstimateRequest {
    ModelInputEstimateRequest {
        model: ModelRef::new("selected", "model"),
        parts: CanonicalMessage::new(
            MessageRole::Assistant,
            vec![ContentPart::Opaque {
                format: "fixture/checkpoint".into(),
                data: serde_json::json!({"encrypted":"untouched"}),
            }],
        )
        .parts,
    }
}

#[tokio::test]
async fn estimate_is_optional_and_does_not_record_model_exchanges() {
    let recorder = Arc::new(CollectingExecutionRecorder::default());
    let scope = ExecutionScope::fresh(Default::default());
    let model = BoundModel::new(
        Arc::new(ModelService::new(Arc::new(Estimator {
            invalid: false,
            pending: false,
        }))),
        ModelExecutionBinding::with_recorder(scope.clone(), recorder.clone()),
        0,
    )
    .with_hooks(
        Arc::new(NoEstimateHooks),
        ExecutionAttribution::detached(scope.execution_id),
        ".".into(),
    );
    let request = estimate_request();
    let result = model
        .estimate_input(request.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.tokens_for(&request.parts[0]), Some(0));
    let facts = recorder.facts.lock().await;
    assert!(facts.requests.is_empty() && facts.responses.is_empty() && facts.errors.is_empty());
    let unsupported = ModelService::new(Arc::new(ImmediateAdapter::new()));
    assert!(unsupported.estimate_input(request).await.unwrap().is_none());
}

#[tokio::test]
async fn estimate_rejects_invalid_provider_identity_and_obeys_cancellation_and_deadline() {
    let service = Arc::new(ModelService::new(Arc::new(Estimator {
        invalid: true,
        pending: false,
    })));
    assert!(
        service
            .estimate_input(estimate_request())
            .await
            .unwrap_err()
            .to_string()
            .contains("unknown or duplicate")
    );
    for cancel in [false, true] {
        let scope = ExecutionScope::fresh(Default::default());
        if cancel {
            scope.cancellation.cancel();
        }
        let model = BoundModel::new(
            Arc::new(ModelService::new(Arc::new(Estimator {
                invalid: false,
                pending: true,
            }))),
            ModelExecutionBinding::detached(scope),
            10,
        );
        let error = model.estimate_input(estimate_request()).await.unwrap_err();
        assert!(
            error
                .downcast_ref::<crate::model_standard::ModelFailure>()
                .is_some()
        );
        assert!(
            error
                .to_string()
                .contains(if cancel { "canceled" } else { "timed out" }),
            "{error:#}"
        );
    }
}

#[tokio::test]
async fn estimate_uses_the_invocation_cancellation_without_canceling_the_execution() {
    let scope = ExecutionScope::fresh(Default::default());
    let model = BoundModel::new(
        Arc::new(ModelService::new(Arc::new(Estimator {
            invalid: false,
            pending: true,
        }))),
        ModelExecutionBinding::detached(scope.clone()),
        10,
    );
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let error = crate::core::model_call_scope::with_operation_scope(
        crate::contracts::OperationOwner::Workflow,
        vec![],
        cancellation,
        model.estimate_input(estimate_request()),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("canceled"));
    assert!(!scope.cancellation.is_cancelled());
}
