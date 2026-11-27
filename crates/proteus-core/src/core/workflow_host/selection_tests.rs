use super::*;
use crate::contracts::{
    AgentWorkflowContext, ApprovalPolicy, CancellationToken, ConversationReader,
    ConversationSnapshot, EventEmitter, ExecutionContext, ExecutionHooks, ExecutionScope,
    HookHandler, HookHost, HookResponse, NoopExecutionRecorder, PolicyContext,
    PolicyVisibilityContext, Tool, ToolContext, ToolRegistry, WorkflowExecutionContext,
};
use crate::domain::{
    PermissionMode, PolicyDecision, ToolSafety, new_session_id, new_thread_id, new_turn_id,
};
use async_trait::async_trait;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct ProbeTool(&'static str, bool);
#[async_trait]
impl Tool for ProbeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            self.0,
            self.0,
            serde_json::json!({"type":"object"}),
            ToolSafety::ReadOnly,
        )
    }
    fn model_visible(&self) -> bool {
        self.1
    }
    async fn invoke(&self, _: &ToolCall, _: ToolContext) -> Result<ToolResult> {
        panic!("selection must not invoke tools")
    }
}
struct Policy;
impl ApprovalPolicy for Policy {
    fn evaluate(&self, _: &ToolCall, _: &PolicyContext) -> PolicyDecision {
        PolicyDecision::Allow
    }
    fn evaluate_visibility(&self, ctx: &PolicyVisibilityContext) -> PolicyDecision {
        if ctx.tool_spec.name == "denied" {
            PolicyDecision::Deny {
                reason: "not available".into(),
            }
        } else {
            PolicyDecision::Allow
        }
    }
}
struct Selector(Arc<Mutex<Vec<HookInput>>>);
#[async_trait]
impl HookHandler for Selector {
    async fn invoke(&self, input: HookInput, _: Arc<dyn HookHost>) -> Result<HookResponse> {
        self.0.lock().unwrap().push(input.clone());
        let HookEvent::BeforeToolSelection { output, .. } = input.event else {
            unreachable!()
        };
        let mut selected = ToolSelectionOutput::new(output.tools);
        selected.metadata = serde_json::json!({"fixture":"selection"});
        Ok(HookResponse::ToolSelection { output: selected })
    }
}
fn host() -> (WorkflowHostRuntime, Arc<Mutex<Vec<HookInput>>>) {
    let scope = ExecutionScope::fresh(CancellationToken::new());
    let captured = Arc::new(Mutex::new(vec![]));
    let hooks: Arc<dyn ExecutionHooks> = Arc::new(crate::core::RuntimeHookChain::new(
        vec![("selector".into(), Arc::new(Selector(captured.clone())))],
        scope.clone(),
        Arc::new(NoopExecutionRecorder),
        None,
    ));
    let mut tools = ToolRegistry::default();
    tools.register(ProbeTool("available", true)).unwrap();
    tools.register(ProbeTool("private", false)).unwrap();
    tools.register(ProbeTool("denied", true)).unwrap();
    let execution = ExecutionContext::new(
        scope,
        0,
        None,
        tools,
        Arc::new(Policy),
        Arc::new(crate::core::HeadlessApprovalTransport),
    )
    .with_hooks(hooks);
    let ctx = WorkflowInvocationContext::Execution(WorkflowExecutionContext {
        execution,
        context: Arc::new(crate::stubs::EmptyContextBuilder),
        model_ref: None,
        reasoning: Default::default(),
        instructions: vec![],
        context_timeout_ms: 1000,
        intent: None,
        permission_mode: PermissionMode::Normal,
    });
    (WorkflowHostRuntime::new(ctx, "/tmp".into()), captured)
}
#[tokio::test]
async fn selection_without_model_or_conversation_filters_private_and_denied_tools() {
    let (host, captured) = host();
    let request =
        ToolSelectionRequest::new(AgentTask::new("select", "/tmp".into())).with_phase("plan");
    let output = host.select_tools(request.clone()).await.unwrap();
    assert_eq!(
        output
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["available"]
    );
    assert_eq!(output.metadata["fixture"], "selection");
    let captured = captured.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert!(captured[0].conversation.is_none());
    assert!(captured[0].attribution.agent.is_none());
    let HookEvent::BeforeToolSelection { input, .. } = &captured[0].event else {
        unreachable!()
    };
    assert_eq!(input.request, request);
    assert_eq!(input.candidates, output.tools);
}
#[tokio::test]
async fn cancelled_selection_does_not_invoke_hooks() {
    let (host, captured) = host();
    host.ctx.execution().scope.cancellation.cancel();
    assert!(
        host.select_tools(ToolSelectionRequest::new(AgentTask::new(
            "select",
            "/tmp".into()
        )))
        .await
        .is_err()
    );
    assert!(captured.lock().unwrap().is_empty());
}

struct CountedHistory(Arc<AtomicUsize>);

#[async_trait]
impl ConversationReader for CountedHistory {
    async fn read(&self) -> Result<ConversationSnapshot> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(ConversationSnapshot {
            revision: 42,
            messages: vec![],
            model_context: vec![],
        })
    }
}

#[tokio::test]
async fn only_active_selection_hooks_read_agent_history() {
    for (configured, subscribed) in [(false, false), (true, false), (true, true)] {
        let (host, captured) = host();
        let mut execution = host.ctx.execution().clone();
        if !subscribed {
            let scope = execution.scope.clone();
            let handlers = if configured {
                vec![(
                    "selector".into(),
                    Arc::new(Selector(captured.clone())) as Arc<dyn HookHandler>,
                )]
            } else {
                vec![]
            };
            let subscriptions =
                serde_json::from_value(serde_json::json!({"selector":["before_context"]})).unwrap();
            execution = execution.with_hooks(Arc::new(
                crate::core::RuntimeHookChain::new(
                    handlers,
                    scope,
                    Arc::new(NoopExecutionRecorder),
                    None,
                )
                .with_subscriptions(subscriptions),
            ));
        }
        let mut agent = AgentWorkflowContext::new(
            execution,
            new_session_id(),
            new_thread_id(),
            new_turn_id(),
            None,
            Default::default(),
            1000,
            Arc::new(EventEmitter::new(Arc::new(
                crate::core::event_store::FanoutEventSink::new(vec![]),
            ))),
            Arc::new(crate::stubs::EmptyContextBuilder),
            Arc::new(crate::core::HeadlessUserInputTransport),
            crate::test_model::no_context_preparation(),
            None,
        );
        let reads = Arc::new(AtomicUsize::new(0));
        agent.conversation = Some(Arc::new(CountedHistory(reads.clone())));
        let host = WorkflowHostRuntime::new(agent.into(), "/tmp".into());
        let output = host
            .select_tools(ToolSelectionRequest::new(AgentTask::new(
                "select",
                "/tmp".into(),
            )))
            .await
            .unwrap();
        assert_eq!(output.tools[0].name, "available");
        assert_eq!(reads.load(Ordering::Relaxed), usize::from(subscribed));
        let captured = captured.lock().unwrap();
        if subscribed {
            assert_eq!(captured[0].conversation.as_ref().unwrap().revision, 42);
        } else {
            assert!(captured.is_empty());
        }
    }
}
