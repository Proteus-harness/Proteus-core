use super::*;
use crate::{
    contracts::{CancellationToken, ModelCallOrigin, OperationOwner},
    core::{AppConfig, HeadlessApprovalTransport, ModelExecutionBinding, RuntimeRegistry},
    domain::{AgentTask, ExchangeId, PermissionMode, new_session_id, new_thread_id, new_turn_id},
    model_standard::{CanonicalModelRequest, CanonicalModelResponse, ModelFailure},
};
use std::sync::Mutex;

#[derive(Debug, PartialEq)]
enum Fact {
    Request(uuid::Uuid, AgentTask),
    Effect,
    Response(uuid::Uuid, ContextBuildOutcome),
}

#[derive(Default)]
struct Recorder {
    facts: Mutex<Vec<Fact>>,
    reject_request: bool,
    reject_response: bool,
}

#[async_trait]
impl ExecutionRecorder for Recorder {
    async fn context_request_recorded(&self, id: uuid::Uuid, task: &AgentTask) -> Result<()> {
        ensure!(!self.reject_request, "request persistence failed");
        self.facts
            .lock()
            .unwrap()
            .push(Fact::Request(id, task.clone()));
        Ok(())
    }
    async fn context_response_recorded(
        &self,
        id: uuid::Uuid,
        outcome: &ContextBuildOutcome,
    ) -> Result<()> {
        ensure!(!self.reject_response, "response persistence failed");
        self.facts
            .lock()
            .unwrap()
            .push(Fact::Response(id, outcome.clone()));
        Ok(())
    }
    async fn model_request_recorded(
        &self,
        _: ExchangeId,
        _: ModelCallOrigin,
        _: OperationOwner,
        _: &CanonicalModelRequest,
    ) -> Result<()> {
        unreachable!()
    }
    async fn model_response_recorded(
        &self,
        _: ExchangeId,
        _: &CanonicalModelResponse,
    ) -> Result<()> {
        unreachable!()
    }
    async fn model_error_recorded(&self, _: ExchangeId, _: &ModelFailure) -> Result<()> {
        unreachable!()
    }
}

enum Behavior {
    Success,
    Error,
    Pending,
}
struct Builder {
    recorder: Arc<Recorder>,
    behavior: Behavior,
    started: tokio::sync::Notify,
}
#[async_trait]
impl ContextBuilder for Builder {
    async fn build(&self, _: ContextBuildInput) -> Result<ContextBundle> {
        self.recorder.facts.lock().unwrap().push(Fact::Effect);
        self.started.notify_one();
        match self.behavior {
            Behavior::Success => {
                Ok(ContextBundle::new(vec![]).with_summary("exact context".into()))
            }
            Behavior::Error => {
                Err(anyhow!("invalid source parameter").context("provider refused input"))
            }
            Behavior::Pending => std::future::pending().await,
        }
    }
}

fn setup(
    behavior: Behavior,
    recorder: Recorder,
    timeout_ms: u64,
) -> (Arc<BoundContext>, Arc<Builder>, ContextBuildInput) {
    let scope = ExecutionScope::fresh(CancellationToken::new());
    let attribution = ExecutionAttribution::detached(scope.execution_id);
    let recorder = Arc::new(recorder);
    let builder = Arc::new(Builder {
        recorder: recorder.clone(),
        behavior,
        started: Default::default(),
    });
    let bound = Arc::new(BoundContext::new(
        builder.clone(),
        scope.clone(),
        attribution,
        recorder,
        timeout_ms,
    ));
    (
        bound,
        builder,
        ContextBuildInput::new(AgentTask::new("input", "/tmp".into()), scope, attribution),
    )
}

fn assert_outcome(recorder: &Recorder, expected: ContextBuildOutcome, effect: bool) {
    let facts = recorder.facts.lock().unwrap();
    let Fact::Request(id, task) = &facts[0] else {
        panic!("request must precede effect")
    };
    assert_eq!(task.text, "input");
    assert_eq!(facts.last(), Some(&Fact::Response(*id, expected)));
    assert_eq!(facts.len(), if effect { 3 } else { 2 });
    if effect {
        assert_eq!(facts[1], Fact::Effect);
    }
}

#[tokio::test]
async fn request_effect_and_exact_success_are_recorded_in_order() {
    let (bound, builder, input) = setup(Behavior::Success, Recorder::default(), 1000);
    let bundle = bound.build(input).await.unwrap();
    assert_outcome(
        &builder.recorder,
        ContextBuildOutcome::Success { bundle },
        true,
    );
}

#[tokio::test]
async fn provider_error_is_recorded_before_returning_it() {
    let (bound, builder, input) = setup(Behavior::Error, Recorder::default(), 1000);
    let error = bound.build(input).await.unwrap_err();
    assert_eq!(error.to_string(), "provider refused input");
    assert_eq!(
        format!("{error:#}"),
        "provider refused input: invalid source parameter"
    );
    assert_outcome(
        &builder.recorder,
        ContextBuildOutcome::Error {
            message: "provider refused input".into(),
            causes: vec!["invalid source parameter".into()],
        },
        true,
    );
}

#[tokio::test]
async fn deadline_records_an_error_instead_of_leaving_a_completed_call_pending() {
    let (bound, builder, input) = setup(Behavior::Pending, Recorder::default(), 1);
    let message = bound.build(input).await.unwrap_err().to_string();
    assert_eq!(message, "context build timed out after 1ms");
    assert_outcome(
        &builder.recorder,
        ContextBuildOutcome::Error {
            message,
            causes: vec![],
        },
        true,
    );
}

#[tokio::test]
async fn spoofed_identity_or_agent_attribution_never_reaches_recorder_or_builder() {
    let (bound, builder, input) = setup(Behavior::Success, Recorder::default(), 1000);
    let mut spoof = input.clone();
    spoof.scope = ExecutionScope::fresh(CancellationToken::new());
    assert!(
        bound
            .build(spoof)
            .await
            .unwrap_err()
            .to_string()
            .contains("execution identity")
    );
    let mut spoof = input;
    spoof.attribution = ExecutionAttribution::for_turn(
        spoof.scope.execution_id,
        new_session_id(),
        new_thread_id(),
        new_turn_id(),
    );
    assert!(
        bound
            .build(spoof)
            .await
            .unwrap_err()
            .to_string()
            .contains("attribution")
    );
    assert!(builder.recorder.facts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn child_cancellation_records_error_without_canceling_parent_execution() {
    let (bound, builder, mut input) = setup(Behavior::Pending, Recorder::default(), 60_000);
    let parent = input.scope.cancellation.clone();
    input.scope = input.scope.child_cancellation_scope();
    let child = input.scope.cancellation.clone();
    let running = tokio::spawn(async move { bound.build(input).await });
    builder.started.notified().await;
    child.cancel();
    let message = running.await.unwrap().unwrap_err().to_string();
    assert_eq!(message, "turn canceled by client");
    assert!(!parent.is_cancelled());
    assert_outcome(
        &builder.recorder,
        ContextBuildOutcome::Error {
            message,
            causes: vec![],
        },
        true,
    );
}

#[tokio::test]
async fn parent_cancellation_cannot_be_bypassed_with_an_unrelated_call_token() {
    let (bound, builder, mut input) = setup(Behavior::Pending, Recorder::default(), 60_000);
    let parent = input.scope.cancellation.clone();
    input.scope.cancellation = CancellationToken::new();
    let running = tokio::spawn(async move { bound.build(input).await });
    builder.started.notified().await;
    parent.cancel();
    let message = running.await.unwrap().unwrap_err().to_string();
    assert_outcome(
        &builder.recorder,
        ContextBuildOutcome::Error {
            message,
            causes: vec![],
        },
        true,
    );
}

#[tokio::test]
async fn already_canceled_execution_records_error_without_starting_effect() {
    let (bound, builder, input) = setup(Behavior::Success, Recorder::default(), 1000);
    input.scope.cancellation.cancel();
    let message = bound.build(input).await.unwrap_err().to_string();
    assert_outcome(
        &builder.recorder,
        ContextBuildOutcome::Error {
            message,
            causes: vec![],
        },
        false,
    );
}

#[tokio::test]
async fn persistence_failure_prevents_effect_or_success_exposure() {
    let (bound, builder, input) = setup(
        Behavior::Success,
        Recorder {
            reject_request: true,
            ..Default::default()
        },
        1000,
    );
    assert_eq!(
        bound.build(input).await.unwrap_err().to_string(),
        "request persistence failed"
    );
    assert!(builder.recorder.facts.lock().unwrap().is_empty());
    let (bound, builder, input) = setup(
        Behavior::Success,
        Recorder {
            reject_response: true,
            ..Default::default()
        },
        1000,
    );
    assert_eq!(
        bound.build(input).await.unwrap_err().to_string(),
        "response persistence failed"
    );
    let facts = builder.recorder.facts.lock().unwrap();
    assert!(matches!(
        facts.as_slice(),
        [Fact::Request(..), Fact::Effect]
    ));
}

#[tokio::test]
async fn dropped_build_leaves_the_request_pending_without_inventing_an_outcome() {
    let (bound, builder, input) = setup(Behavior::Pending, Recorder::default(), 60_000);
    let running = tokio::spawn(async move { bound.build(input).await });
    builder.started.notified().await;
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    assert!(matches!(
        builder.recorder.facts.lock().unwrap().as_slice(),
        [Fact::Request(..), Fact::Effect]
    ));
}

#[tokio::test]
async fn standalone_registry_factory_binds_direct_and_host_deadlines_without_model() {
    let workspace = tempfile::tempdir().unwrap();
    let mut config: AppConfig = serde_json::from_value(serde_json::json!({})).unwrap();
    config.runtime.context_timeout_ms = 1;
    let mut registry = RuntimeRegistry::from_catalog(
        &config,
        workspace.path().into(),
        crate::core::ModuleCatalog::new(),
    )
    .unwrap();
    let recorder = Arc::new(Recorder::default());
    registry.context = Arc::new(Builder {
        recorder: recorder.clone(),
        behavior: Behavior::Pending,
        started: Default::default(),
    });
    let scope = ExecutionScope::fresh(CancellationToken::new());
    let attribution = ExecutionAttribution::detached(scope.execution_id);
    let ctx = registry
        .workflow_execution_context(
            scope.clone(),
            Arc::new(HeadlessApprovalTransport),
            PermissionMode::Normal,
        )
        .unwrap();
    assert!(ctx.execution().model.is_none());
    assert!(ctx.agent().is_err());
    let task = AgentTask::new("input", workspace.path().into());
    assert_eq!(
        ctx.context()
            .build(ContextBuildInput::new(task.clone(), scope, attribution))
            .await
            .unwrap_err()
            .to_string(),
        "context build timed out after 1ms"
    );
    let host = crate::core::workflow_host::WorkflowHostRuntime::new(ctx, workspace.path().into());
    assert_eq!(
        host.build_context(task).await.unwrap_err().to_string(),
        "context build timed out after 1ms"
    );
    assert_eq!(recorder.facts.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn registry_binding_reuses_supplied_execution_recorder_without_a_model() {
    let workspace = tempfile::tempdir().unwrap();
    let registry = RuntimeRegistry::from_catalog(
        &serde_json::from_value(serde_json::json!({})).unwrap(),
        workspace.path().into(),
        crate::core::ModuleCatalog::new(),
    )
    .unwrap();
    let recorder = Arc::new(Recorder::default());
    let scope = ExecutionScope::fresh(CancellationToken::new());
    let attribution = ExecutionAttribution::detached(scope.execution_id);
    let context = registry.bind_context(&ModelExecutionBinding::with_recorder(
        scope.clone(),
        recorder.clone(),
    ));
    let bundle = context
        .build(ContextBuildInput::new(
            AgentTask::new("input", workspace.path().into()),
            scope,
            attribution,
        ))
        .await
        .unwrap();
    assert_outcome(&recorder, ContextBuildOutcome::Success { bundle }, false);
}
