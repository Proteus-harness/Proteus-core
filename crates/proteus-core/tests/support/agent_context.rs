//! Shared execution context for real process-agent boundary tests.

use std::{path::PathBuf, sync::Arc};

use proteus_contracts::{
    contracts::{
        AgentWorkflowContext, ApprovalPolicy, CancellationToken, EventEmitter, ExecutionScope,
        PolicyContext, PolicyVisibilityContext,
    },
    domain::{
        ModelRef, PermissionMode, PolicyDecision, ReasoningConfig, ToolCall, new_session_id,
        new_thread_id, new_turn_id,
    },
};
use proteus_core::core::{
    HeadlessApprovalTransport, HeadlessUserInputTransport, InMemoryEventStore,
    ModelExecutionBinding, RuntimeRegistry,
};

use crate::test_model;

struct AllowAllPolicy;

impl ApprovalPolicy for AllowAllPolicy {
    fn evaluate(&self, _call: &ToolCall, _ctx: &PolicyContext) -> PolicyDecision {
        PolicyDecision::Allow
    }

    fn evaluate_visibility(&self, _ctx: &PolicyVisibilityContext) -> PolicyDecision {
        PolicyDecision::Allow
    }
}

pub fn test_runtime_context(events: Arc<InMemoryEventStore>) -> AgentWorkflowContext {
    let registry = RuntimeRegistry::from_config(&test_model::config(), PathBuf::from("."))
        .expect("default runtime registry");
    let mut execution = registry
        .execution_context(
            ModelExecutionBinding::detached(ExecutionScope::fresh(CancellationToken::new())),
            Arc::new(HeadlessApprovalTransport),
            PermissionMode::Normal,
        )
        .unwrap();
    execution.policy = Arc::new(AllowAllPolicy);
    AgentWorkflowContext::new(
        execution,
        new_session_id(),
        new_thread_id(),
        new_turn_id(),
        Some(ModelRef::new("fake", "fake-tool-model")),
        ReasoningConfig::default(),
        30_000,
        Arc::new(EventEmitter::new(events)),
        registry.context.clone(),
        Arc::new(HeadlessUserInputTransport),
        test_model::no_context_preparation(),
        None,
    )
}
