//! Ordered host-owned hook execution and durable accepted boundary facts.
use crate::{
    contracts::{
        CancellationToken, ExecutionHooks, ExecutionRecorder, ExecutionScope, HookEvent,
        HookHandler, HookInput, HookStep, HookStepOutcome, HookTrace, apply_hook_response,
    },
    core::ModelService,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::sync::{Arc, OnceLock};

pub struct RuntimeHookChain {
    handlers: Vec<(String, Arc<dyn HookHandler>)>,
    scope: ExecutionScope,
    recorder: Arc<dyn ExecutionRecorder>,
    model_service: Option<Arc<ModelService>>,
    operations: OnceLock<super::extension_operations::ExtensionOperations>,
}
impl RuntimeHookChain {
    pub fn new(
        handlers: Vec<(String, Arc<dyn HookHandler>)>,
        scope: ExecutionScope,
        recorder: Arc<dyn ExecutionRecorder>,
        model_service: Option<Arc<ModelService>>,
    ) -> Self {
        Self {
            handlers,
            scope,
            recorder,
            model_service,
            operations: OnceLock::new(),
        }
    }
    pub(crate) fn bind_operations(
        &self,
        operations: super::extension_operations::ExtensionOperations,
    ) {
        assert!(
            self.operations.set(operations).is_ok(),
            "hook operations already bound"
        );
    }
}
#[async_trait]
impl ExecutionHooks for RuntimeHookChain {
    fn is_active(&self) -> bool {
        !self.handlers.is_empty()
    }
    async fn apply(&self, input: HookInput) -> Result<HookEvent> {
        if input.attribution.execution_id != self.scope.execution_id {
            anyhow::bail!("hook attribution conflicts with bound execution scope");
        }
        if self.handlers.is_empty()
            && !matches!(&input.event, HookEvent::BeforeToolSelection { .. })
        {
            return Ok(input.event);
        }
        let notification = matches!(
            &input.event,
            HookEvent::TurnStarted { .. } | HookEvent::TurnSettled { .. }
        );
        let cleanup = matches!(&input.event, HookEvent::TurnSettled { .. });
        let cancellation = if cleanup {
            CancellationToken::new()
        } else {
            super::model_call_scope::operation_cancellation(&self.scope.cancellation).child_token()
        };
        let mut trace = HookTrace {
            owner: super::model_call_scope::current_operation_owner(),
            input: input.clone(),
            steps: vec![],
            output: None,
        };
        let mut event = input.event.clone();
        for (module_id, handler) in &self.handlers {
            let mut invocation = input.clone();
            invocation.event = event.clone();
            let invocation_id = uuid::Uuid::new_v4();
            let host = Arc::new(super::extension_operations::InvocationOperations {
                operations: self.operations.get().cloned(),
                owner: crate::contracts::OperationOwner::Hook { invocation_id },
                lineage: super::model_call_scope::current_tool_lineage(),
                cancellation: cancellation.child_token(),
                cwd: input.cwd.clone(),
                model_allowed: !notification,
                tools_allowed: !notification
                    && !matches!(
                        &event,
                        HookEvent::BeforeTool { .. } | HookEvent::AfterTool { .. }
                    ),
            });
            let outcome = if cancellation.is_cancelled() {
                Err(anyhow!("hook execution canceled"))
            } else {
                let handler_cancellation = host.cancellation.clone();
                let invocation = super::model_call_scope::with_operation_scope(
                    crate::contracts::OperationOwner::Hook { invocation_id },
                    host.lineage.clone(),
                    handler_cancellation.clone(),
                    handler.invoke(invocation, host),
                );
                tokio::pin!(invocation);
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {
                        handler_cancellation.cancel();
                        let _ = tokio::time::timeout(std::time::Duration::from_millis(500), &mut invocation).await;
                        Err(anyhow!("hook execution canceled"))
                    },
                    result = &mut invocation => result,
                }
            };
            let validated = outcome.and_then(|response| {
                // Parts removed by an earlier contribution remain immutable at this boundary.
                if matches!(
                    &input.event,
                    HookEvent::BeforeModel { .. } | HookEvent::BeforeToolSelection { .. }
                ) {
                    apply_hook_response(&input.event, &response)?;
                }
                let mut next = apply_hook_response(&event, &response)?;
                if let HookEvent::BeforeModel { request, .. } = &mut next {
                    *request = self
                        .model_service
                        .as_ref()
                        .ok_or_else(|| anyhow!("no model is configured"))?
                        .prepare_request(request.clone())?;
                }
                Ok((response, next))
            });
            match validated {
                Ok((response, next)) => {
                    trace.steps.push(HookStep {
                        invocation_id,
                        module_id: module_id.clone(),
                        outcome: HookStepOutcome::Accepted { response },
                    });
                    event = next;
                    if matches!(
                        &event,
                        HookEvent::BeforeTool {
                            blocked: Some(_),
                            ..
                        } | HookEvent::BeforeStop {
                            continuation: Some(_),
                            ..
                        }
                    ) {
                        break;
                    }
                }
                Err(error) => {
                    // Replay reproduces this complete canonical failure without rebuilding
                    // process/adapter error sources.
                    let model_failure = error
                        .downcast_ref::<crate::model_standard::ModelFailure>()
                        .cloned();
                    let error = match &model_failure {
                        Some(failure) => anyhow::Error::new(failure.clone()),
                        None => anyhow!(format!("{error:#}")),
                    };
                    trace.steps.push(HookStep {
                        invocation_id,
                        module_id: module_id.clone(),
                        outcome: HookStepOutcome::Failed {
                            message: error.to_string(),
                            model_failure,
                        },
                    });
                    if !notification {
                        self.recorder.hook_recorded(&trace).await?;
                        return Err(error);
                    }
                }
            }
        }
        trace.output = Some(event.clone());
        self.recorder.hook_recorded(&trace).await?;
        Ok(event)
    }
}

#[cfg(test)]
#[path = "execution_hooks_tests.rs"]
mod tests;
