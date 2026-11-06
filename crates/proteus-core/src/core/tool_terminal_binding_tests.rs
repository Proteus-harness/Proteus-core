//! Real registry/operation binding: direct, nested-tool and hook publication.
use super::*;
use crate::{
    contracts::*,
    core::{
        HeadlessApprovalTransport, HeadlessUserInputTransport, InMemoryEventStore, ToolOrchestrator,
    },
    domain::*,
};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Mutex;

#[derive(Default)]
struct Facts {
    calls: Mutex<Vec<(ToolCall, ExecutionAttribution, OperationOwner)>>,
    results: Mutex<Vec<ToolResult>>,
}
#[async_trait]
impl ToolExecutionRecorder for Facts {
    async fn tool_call_requested(
        &self,
        attribution: ExecutionAttribution,
        owner: OperationOwner,
        call: &ToolCall,
    ) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((call.clone(), attribution, owner));
        Ok(())
    }
    async fn tool_call_resolved(
        &self,
        _: ExecutionAttribution,
        _: &ToolCall,
        _: &ToolCallResolution,
    ) -> Result<()> {
        Ok(())
    }
    async fn tool_approval_requested(
        &self,
        _: ExecutionAttribution,
        _: &ToolCall,
        _: &str,
    ) -> Result<()> {
        Ok(())
    }
    async fn tool_result_recorded(
        &self,
        _: ExecutionAttribution,
        result: &ToolResult,
    ) -> Result<()> {
        self.results.lock().unwrap().push(result.clone());
        Ok(())
    }
}

struct Controller;
#[async_trait]
impl ToolTerminalController for Controller {
    async fn control(&self, _: ToolTerminalAction) -> Result<ToolTerminalResponse> {
        Ok(ToolTerminalResponse::Accepted)
    }
}
#[derive(Default)]
struct Terminals(Mutex<Vec<ToolTerminalRef>>);
#[async_trait]
impl ToolTerminalHost for Terminals {
    async fn publish(
        &self,
        descriptor: ToolTerminalDescriptor,
        _: Arc<dyn ToolTerminalController>,
    ) -> Result<ToolTerminalRef> {
        let reference = ToolTerminalRef {
            id: uuid::Uuid::new_v4(),
            title: descriptor.handle,
            tty: descriptor.tty,
        };
        self.0.lock().unwrap().push(reference.clone());
        Ok(reference)
    }
}

struct Policy {
    deny_leaf: bool,
}
impl ApprovalPolicy for Policy {
    fn evaluate(&self, call: &ToolCall, _: &PolicyContext) -> PolicyDecision {
        if self.deny_leaf && call.name == "leaf" {
            PolicyDecision::Deny {
                reason: "fixture denied leaf".into(),
            }
        } else {
            PolicyDecision::Allow
        }
    }
    fn evaluate_visibility(&self, _: &PolicyVisibilityContext) -> PolicyDecision {
        PolicyDecision::Allow
    }
}

struct Leaf {
    wait: bool,
}
#[async_trait]
impl Tool for Leaf {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "leaf",
            "terminal fixture",
            json!({"type":"object", "properties":{"effective":{"type":"boolean"}},"additionalProperties":false}),
            ToolSafety::RunsCommands,
        )
    }
    async fn invoke(&self, call: &ToolCall, ctx: ToolContext) -> Result<ToolResult> {
        assert_eq!(
            call.args["effective"], true,
            "BeforeTool arguments must reach the actual invocation"
        );
        let Some(host) = ctx.terminal_host else {
            return Ok(ToolResult::ok(
                call.id.clone(),
                "ordinary output without terminal host",
            ));
        };
        let terminal = host
            .publish(
                ToolTerminalDescriptor {
                    handle: call.id.clone(),
                    title: "fixture".into(),
                    tty: true,
                    columns: 80,
                    rows: 24,
                },
                Arc::new(Controller),
            )
            .await?;
        if self.wait {
            ctx.cancellation.cancelled().await;
            anyhow::bail!("leaf observed inherited cancellation");
        }
        Ok(ToolResult::ok(call.id.clone(), "ordinary output")
            .with_metadata(json!({"tool_terminal":terminal})))
    }
}
struct Parent;
#[async_trait]
impl Tool for Parent {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "parent",
            "nested fixture",
            json!({"type":"object","additionalProperties":false}),
            ToolSafety::RunsCommands,
        )
    }
    async fn invoke(&self, call: &ToolCall, ctx: ToolContext) -> Result<ToolResult> {
        let child = ctx
            .host
            .unwrap()
            .execute_tool(ToolCall::new("nested-child", "leaf", json!({})))
            .await?;
        if !child.ok {
            return Ok(ToolResult::error(call.id.clone(), child.text_or_status()));
        }
        Ok(ToolResult::ok(call.id.clone(), child.text_or_status()))
    }
}
struct Hook;
#[async_trait]
impl HookHandler for Hook {
    async fn invoke(&self, input: HookInput, host: Arc<dyn HookHost>) -> Result<HookResponse> {
        match input.event {
            HookEvent::BeforeTool { call, .. } if call.name == "leaf" => {
                // Hook contract replaces arguments, not call identity.
                Ok(HookResponse::ToolArguments {
                    args: json!({"effective":true}),
                })
            }
            HookEvent::BeforeStop { .. } => {
                let result = host
                    .execute_tool(ToolCall::new("hook-child", "leaf", json!({})))
                    .await?;
                assert!(result.ok);
                Ok(HookResponse::Continue)
            }
            _ => Ok(HookResponse::Continue),
        }
    }
}

fn registry(wait: bool, deny_leaf: bool) -> RuntimeRegistry {
    let mut tools = ToolRegistry::new();
    for tool in [Arc::new(Parent) as Arc<dyn Tool>, Arc::new(Leaf { wait })] {
        tools
            .register_arc(
                ToolSource::Dynamic {
                    origin: "fixture".into(),
                },
                tool,
            )
            .unwrap();
    }
    RuntimeRegistry {
        config_exports: vec![],
        hooks: vec![("fixture".into(), Arc::new(Hook))],
        cwd: "/workspace".into(),
        model_config: None,
        runtime_config: Default::default(),
        instructions: vec![],
        model_service: None,
        context: Arc::new(EmptyContextBuilder),
        tools,
        process_tool_specs: vec![],
        policy: Arc::new(Policy { deny_leaf }),
        agent_control: None,
        workflow: Arc::new(NoWorkflow),
        context_providers: vec![],
        mcp_servers: vec![],
        plugins: vec![],
    }
}

fn context(
    registry: &RuntimeRegistry,
    facts: Arc<Facts>,
    terminals: Option<Arc<Terminals>>,
    sink: Arc<InMemoryEventStore>,
    cancellation: CancellationToken,
) -> AgentWorkflowContext {
    let events = Arc::new(EventEmitter::new(sink));
    let (session, thread, turn) = (new_session_id(), new_thread_id(), new_turn_id());
    let mut binding = ModelExecutionBinding::for_turn(
        ExecutionScope::fresh(cancellation),
        events.clone(),
        session,
        thread,
        turn,
        Arc::new(NoopExecutionRecorder),
    )
    .with_tool_recorder(facts.clone());
    if let Some(terminals) = &terminals {
        binding = binding.with_tool_terminal_registry(terminals.clone());
    }
    let bound_context = registry.bind_context(&binding);
    let execution = registry
        .execution_context(
            binding,
            Arc::new(HeadlessApprovalTransport),
            PermissionMode::Normal,
        )
        .unwrap();
    let mut ctx = registry
        .agent_workflow_context(
            execution,
            bound_context,
            session,
            thread,
            turn,
            None,
            Default::default(),
            events,
            Arc::new(HeadlessUserInputTransport),
        )
        .with_tool_recorder(facts);
    ctx.terminal_registry = terminals.map(|host| host as Arc<dyn ToolTerminalHost>);
    ctx
}

#[tokio::test]
async fn direct_nested_and_hook_terminals_use_exact_call_ids_and_one_live_row() {
    let registry = registry(false, false);
    let sink = Arc::new(InMemoryEventStore::new());
    let facts = Arc::new(Facts::default());
    let terminals = Arc::new(Terminals::default());
    let ctx = context(
        &registry,
        facts.clone(),
        Some(terminals.clone()),
        sink.clone(),
        CancellationToken::new(),
    );
    let task = AgentTask::new("fixture", registry.cwd.clone());
    let orchestrator = ToolOrchestrator::default();
    let direct = orchestrator
        .execute(&ctx, &task, ToolCall::new("direct", "leaf", json!({})))
        .await
        .unwrap();
    let parent = orchestrator
        .execute(&ctx, &task, ToolCall::new("outer", "parent", json!({})))
        .await
        .unwrap();
    assert!(direct.ok && parent.ok);
    assert_eq!(parent.output, "ordinary output");
    assert!(parent.metadata.get("tool_terminal").is_none());
    ctx.execution
        .hooks
        .apply(HookInput {
            event: HookEvent::BeforeStop {
                task,
                history: vec![],
                output: AgentOutput::text("done"),
                attempt: 0,
                continuation: None,
            },
            attribution: ExecutionAttribution::for_turn(
                ctx.execution.scope.execution_id,
                ctx.session_id,
                ctx.thread_id,
                ctx.turn_id,
            ),
            cwd: registry.cwd.clone(),
            conversation: None,
        })
        .await
        .unwrap();

    let events = sink.envelopes().await;
    for id in ["direct", "nested-child", "hook-child"] {
        let requested: Vec<_> = events.iter().enumerate().filter(|(_, envelope)| matches!(&envelope.event, Event::ToolCallRequested { call } if call.id == id)).collect();
        let published: Vec<_> = events.iter().enumerate().filter(|(_, envelope)| matches!(&envelope.event, Event::ToolTerminalBound { call_id, .. } if call_id == id)).collect();
        let finished: Vec<_> = events.iter().enumerate().filter(|(_, envelope)| matches!(&envelope.event, Event::ToolFinished { result } if result.call_id == id)).collect();
        assert_eq!(
            (requested.len(), published.len(), finished.len()),
            (1, 1, 1),
            "one live row and binding for {id}"
        );
        assert!(requested[0].0 < published[0].0 && published[0].0 < finished[0].0);
        for (_, envelope) in [requested[0], published[0], finished[0]] {
            assert_eq!(
                (envelope.session_id, envelope.thread_id, envelope.turn_id),
                (ctx.session_id, ctx.thread_id, Some(ctx.turn_id))
            );
        }
        let result = facts
            .results
            .lock()
            .unwrap()
            .iter()
            .find(|result| result.call_id == id)
            .unwrap()
            .clone();
        let terminal: ToolTerminalRef =
            serde_json::from_value(result.metadata["tool_terminal"].clone()).unwrap();
        assert_eq!(
            terminal.title, id,
            "recorded metadata belongs to the real nested invocation"
        );
    }
    assert!(!events.iter().any(|envelope| matches!(&envelope.event, Event::ToolTerminalBound { call_id, .. } if call_id == "outer")));
    let calls = facts.calls.lock().unwrap();
    assert!(calls.iter().all(|(_, attribution, _)| attribution.execution_id == ctx.execution.scope.execution_id));
    assert!(
        calls
            .iter()
            .any(|(call, _, owner)| call.id == "nested-child"
                && *owner
                    == OperationOwner::Tool {
                        call_id: "outer".into()
                    })
    );
    assert!(
        calls.iter().any(|(call, _, owner)| call.id == "hook-child"
            && matches!(owner, OperationOwner::Hook { .. }))
    );
}

#[tokio::test]
async fn absent_terminal_host_and_policy_denial_keep_nested_output_and_authority() {
    for denied in [false, true] {
        let registry = registry(false, denied);
        let facts = Arc::new(Facts::default());
        let sink = Arc::new(InMemoryEventStore::new());
        let ctx = context(
            &registry,
            facts,
            None,
            sink.clone(),
            CancellationToken::new(),
        );
        let result = ToolOrchestrator::default()
            .execute(
                &ctx,
                &AgentTask::new("fixture", registry.cwd.clone()),
                ToolCall::new("outer", "parent", json!({})),
            )
            .await
            .unwrap();
        assert_eq!(result.ok, !denied);
        assert!(result.text_or_status().contains(if denied {
            "fixture denied leaf"
        } else {
            "ordinary output without terminal host"
        }));
        assert!(
            !sink
                .events()
                .await
                .iter()
                .any(|event| matches!(event, Event::ToolTerminalBound { .. }))
        );
    }
}

#[tokio::test]
async fn nested_terminal_execution_observes_parent_cancellation() {
    let registry = registry(true, false);
    let facts = Arc::new(Facts::default());
    let sink = Arc::new(InMemoryEventStore::new());
    let terminals = Arc::new(Terminals::default());
    let cancellation = CancellationToken::new();
    let ctx = context(
        &registry,
        facts.clone(),
        Some(terminals.clone()),
        sink.clone(),
        cancellation.clone(),
    );
    let run = tokio::spawn(async move {
        ToolOrchestrator::default()
            .execute(
                &ctx,
                &AgentTask::new("fixture", "/workspace".into()),
                ToolCall::new("outer", "parent", json!({})),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while terminals.0.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancellation.cancel();
    let error = tokio::time::timeout(std::time::Duration::from_secs(2), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    // Cancellation also prevents AfterTool hooks. The caller receives that
    // terminal error while the settled child result must still be recorded.
    assert!(error.to_string().contains("canceled"), "{error:#}");
    assert!(
        facts
            .results
            .lock()
            .unwrap()
            .iter()
            .any(|result| result.call_id == "nested-child" && !result.ok)
    );
    let events = sink.events().await;
    assert_eq!(events.iter().filter(|event| matches!(event, Event::ToolTerminalBound { call_id, .. } if call_id == "nested-child")).count(), 1);
    assert!(events.iter().any(|event| matches!(event, Event::ToolFinished { result } if result.call_id == "nested-child" && !result.ok)));
}
