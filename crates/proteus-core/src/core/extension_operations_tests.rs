use super::*;
use crate::{
    contracts::*,
    core::{
        BoundModel, ModelExecutionBinding, ModelService, RuntimeHookChain, ToolExecutionBinding,
    },
    domain::{PolicyDecision, *},
    model_standard::*,
};
use serde_json::json;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Default)]
struct Facts {
    requests: Mutex<Vec<(ModelCallOrigin, OperationOwner)>>,
    tools: Mutex<Vec<(String, OperationOwner)>>,
    traces: Mutex<Vec<HookTrace>>,
    errors: AtomicUsize,
}
#[async_trait]
impl ExecutionRecorder for Facts {
    async fn model_request_recorded(
        &self,
        _: ExchangeId,
        origin: ModelCallOrigin,
        owner: OperationOwner,
        _: &CanonicalModelRequest,
    ) -> Result<()> {
        self.requests.lock().unwrap().push((origin, owner));
        Ok(())
    }
    async fn model_response_recorded(
        &self,
        _: ExchangeId,
        _: &CanonicalModelResponse,
    ) -> Result<()> {
        Ok(())
    }
    async fn model_error_recorded(&self, _: ExchangeId, _: &ModelFailure) -> Result<()> {
        self.errors.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn hook_recorded(&self, trace: &HookTrace) -> Result<()> {
        self.traces.lock().unwrap().push(trace.clone());
        Ok(())
    }
}
#[async_trait]
impl ToolExecutionRecorder for Facts {
    async fn tool_call_requested(
        &self,
        _: ExecutionAttribution,
        owner: OperationOwner,
        call: &ToolCall,
    ) -> Result<()> {
        self.tools.lock().unwrap().push((call.name.clone(), owner));
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
    async fn tool_call_resolved(
        &self,
        _: ExecutionAttribution,
        _: &ToolCall,
        _: &ToolCallResolution,
    ) -> Result<()> {
        Ok(())
    }
    async fn tool_result_recorded(&self, _: ExecutionAttribution, _: &ToolResult) -> Result<()> {
        Ok(())
    }
}
struct Provider {
    calls: AtomicUsize,
    wait: bool,
}
#[async_trait]
impl Model for Provider {
    fn id(&self) -> std::borrow::Cow<'static, str> {
        "fixture".into()
    }
    fn capabilities(&self, _: &ModelRef) -> Result<ModelCapabilities> {
        Ok(ModelCapabilities::empty())
    }
    async fn stream(&self, _: CanonicalModelRequest) -> Result<ModelEventStream> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.wait {
            std::future::pending::<()>().await;
        }
        Ok(Box::pin(futures_util::stream::iter([Ok(
            ModelStreamEvent::Response {
                response: CanonicalModelResponse::new(
                    CanonicalMessage::text(MessageRole::Assistant, "helper"),
                    vec![],
                    FinishReason::Stop,
                ),
            },
        )])))
    }
}
fn request() -> CanonicalModelRequest {
    CanonicalModelRequest::new(ModelRef::new("fixture", "x"), vec![])
}
struct HelperHook {
    calls: AtomicUsize,
}
#[async_trait]
impl HookHandler for HelperHook {
    async fn invoke(&self, input: HookInput, host: Arc<dyn HookHost>) -> Result<HookResponse> {
        match input.event {
            HookEvent::BeforeModel { .. } => {
                self.calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(
                    host.complete_model(request()).await?.messages[0].display_text(),
                    "helper"
                );
            }
            HookEvent::BeforeTool { .. } | HookEvent::AfterTool { .. } => {
                assert!(
                    host.execute_tool(ToolCall::new(new_call_id(), "child", json!({})))
                        .await
                        .unwrap_err()
                        .to_string()
                        .contains("unavailable")
                );
            }
            HookEvent::TurnStarted { .. } | HookEvent::TurnSettled { .. } => {
                assert!(host.complete_model(request()).await.is_err());
                assert!(
                    host.execute_tool(ToolCall::new(new_call_id(), "child", json!({})))
                        .await
                        .is_err()
                );
            }
            _ => {}
        }
        Ok(HookResponse::Continue)
    }
}
struct Snapshot;
#[async_trait]
impl ConversationReader for Snapshot {
    async fn read(&self) -> Result<ConversationSnapshot> {
        Ok(ConversationSnapshot {
            revision: 9,
            messages: vec![CanonicalMessage::text(
                MessageRole::User,
                "read-only parent snapshot",
            )],
            model_context: vec![],
        })
    }
}
struct AskChild;
impl ApprovalPolicy for AskChild {
    fn evaluate(&self, call: &ToolCall, _: &PolicyContext) -> PolicyDecision {
        if call.name == "child" {
            PolicyDecision::Ask {
                reason: "child effect".into(),
            }
        } else {
            PolicyDecision::Allow
        }
    }
    fn evaluate_visibility(&self, _: &PolicyVisibilityContext) -> PolicyDecision {
        PolicyDecision::Allow
    }
}
struct Approval {
    calls: AtomicUsize,
    approved: bool,
}
#[async_trait]
impl ApprovalTransport for Approval {
    fn can_request_approval(&self) -> bool {
        true
    }
    async fn request_approval(&self, _: ApprovalRequest) -> Result<ApprovalResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(if self.approved {
            ApprovalResponse::approve()
        } else {
            ApprovalResponse::deny("denied")
        })
    }
}
struct Nested {
    name: &'static str,
    next: Option<&'static str>,
    effects: Arc<AtomicUsize>,
}
#[async_trait]
impl Tool for Nested {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            self.name,
            "fixture",
            json!({"type":"object","additionalProperties":false}),
            ToolSafety::RunsCommands,
        )
    }
    async fn invoke(&self, call: &ToolCall, ctx: ToolContext) -> Result<ToolResult> {
        if let Some(next) = self.next {
            return ctx
                .host
                .unwrap()
                .execute_tool(ToolCall::new(new_call_id(), next, json!({})))
                .await
                .map(|mut result| {
                    result.call_id = call.id.clone();
                    result
                });
        }
        let snapshot = ctx
            .conversation
            .expect("nested snapshot inherited")
            .read()
            .await?;
        assert_eq!(
            snapshot.messages[0].display_text(),
            "read-only parent snapshot"
        );
        assert!(
            snapshot
                .for_tool(call)
                .unwrap_err()
                .to_string()
                .contains("no checkpointed conversation message")
        );
        assert!(ctx.conversation_session_id.is_some());
        self.effects.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::ok(call.id.clone(), "child"))
    }
}
fn bind(
    scope: ExecutionScope,
    provider: Arc<Provider>,
    facts: Arc<Facts>,
    registry: ToolRegistry,
    approval: Arc<Approval>,
    hook: Arc<HelperHook>,
) -> (
    Arc<RuntimeHookChain>,
    Arc<dyn ToolHost>,
    Arc<dyn Model>,
    BoundTools,
) {
    let service = Arc::new(ModelService::new(provider));
    let chain = Arc::new(RuntimeHookChain::new(
        vec![("helper".into(), hook)],
        scope.clone(),
        facts.clone(),
        Some(service.clone()),
    ));
    let hooks: Arc<dyn ExecutionHooks> = chain.clone();
    let tools = BoundTools::new(
        registry,
        Arc::new(AskChild),
        approval,
        Arc::default(),
        ToolExecutionBinding::detached(scope.clone()).with_recorder(facts.clone()),
    )
    .with_conversation(Some(Arc::new(Snapshot)), Some(new_session_id()));
    let model: Arc<dyn Model> = Arc::new(
        BoundModel::new(
            service.clone(),
            ModelExecutionBinding::with_recorder(scope.clone(), facts.clone()),
            1_000,
        )
        .with_tool_authority(tools.clone()),
    );
    let ops = Arc::new_cyclic(|weak| InvocationOperations {
        operations: Some(ExtensionOperations {
            model: Some(model),
            tools: tools
                .clone()
                .with_hooks(Arc::new(WeakExecutionHooks(Arc::downgrade(&hooks))))
                .with_operations(Some(Arc::new(WeakToolHost(weak.clone())))),
        }),
        owner: OperationOwner::Workflow,
        lineage: vec![],
        cancellation: scope.cancellation.clone(),
        cwd: "/tmp".into(),
        model_allowed: true,
        tools_allowed: true,
    });
    chain.bind_operations(ops.operations.as_ref().unwrap().clone());
    let root: Arc<dyn Model> = Arc::new(
        BoundModel::new(
            service,
            ModelExecutionBinding::with_recorder(scope, facts),
            1_000,
        )
        .with_hooks(hooks.clone(), tools.binding().attribution(), "/tmp".into()),
    );
    let bound = tools.with_hooks(hooks).with_operations(Some(ops.clone()));
    (chain, ops, root, bound)
}
#[tokio::test]
async fn auxiliary_model_skips_recursive_hook_and_records_actual_hook_owner() {
    let facts = Arc::new(Facts::default());
    let hook = Arc::new(HelperHook {
        calls: AtomicUsize::new(0),
    });
    let provider = Arc::new(Provider {
        calls: AtomicUsize::new(0),
        wait: false,
    });
    let (_chain, _ops, root, _tools) = bind(
        ExecutionScope::fresh(CancellationToken::new()),
        provider.clone(),
        facts.clone(),
        ToolRegistry::new(),
        Arc::new(Approval {
            calls: AtomicUsize::new(0),
            approved: true,
        }),
        hook.clone(),
    );
    root.complete(request()).await.unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert_eq!(hook.calls.load(Ordering::SeqCst), 1);
    let invocation_id = facts.traces.lock().unwrap()[0].steps[0].invocation_id;
    assert_eq!(
        *facts.requests.lock().unwrap(),
        vec![
            (
                ModelCallOrigin::Auxiliary,
                OperationOwner::Hook { invocation_id }
            ),
            (ModelCallOrigin::Direct, OperationOwner::Workflow)
        ]
    );
}
#[tokio::test]
async fn nested_tools_honor_approval_and_reject_registry_cycles_with_new_call_ids() {
    for approved in [true, false] {
        let effects = Arc::new(AtomicUsize::new(0));
        let mut registry = ToolRegistry::new();
        for (name, next) in [
            ("parent", Some("child")),
            ("child", None),
            ("cycle", Some("cycle")),
        ] {
            registry
                .register_arc(
                    ToolSource::Dynamic {
                        origin: "fixture".into(),
                    },
                    Arc::new(Nested {
                        name,
                        next,
                        effects: effects.clone(),
                    }),
                )
                .unwrap();
        }
        let approval = Arc::new(Approval {
            calls: AtomicUsize::new(0),
            approved,
        });
        let facts = Arc::new(Facts::default());
        let (_chain, _ops, _root, tools) = bind(
            ExecutionScope::fresh(CancellationToken::new()),
            Arc::new(Provider {
                calls: AtomicUsize::new(0),
                wait: false,
            }),
            facts.clone(),
            registry,
            approval.clone(),
            Arc::new(HelperHook {
                calls: AtomicUsize::new(0),
            }),
        );
        let parent_id = new_call_id();
        let result = tools
            .execute(
                "/tmp".into(),
                ToolCall::new(parent_id.clone(), "parent", json!({})),
            )
            .await
            .unwrap();
        assert_eq!(result.ok, approved);
        assert_eq!(effects.load(Ordering::SeqCst), usize::from(approved));
        assert_eq!(approval.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            *facts.tools.lock().unwrap(),
            vec![
                ("parent".into(), OperationOwner::Workflow),
                ("child".into(), OperationOwner::Tool { call_id: parent_id })
            ]
        );
        let cyclic = tools
            .execute(
                "/tmp".into(),
                ToolCall::new(new_call_id(), "cycle", json!({})),
            )
            .await
            .unwrap();
        assert!(!cyclic.ok);
        assert!(cyclic.text_or_status().contains("nested tool cycle"));
    }
}
#[tokio::test]
async fn canceled_helper_records_terminal_error_under_parent_owner() {
    let cancellation = CancellationToken::new();
    let scope = ExecutionScope::fresh(cancellation.clone());
    let facts = Arc::new(Facts::default());
    let provider = Arc::new(Provider {
        calls: AtomicUsize::new(0),
        wait: true,
    });
    let (_chain, ops, _root, _tools) = bind(
        scope,
        provider.clone(),
        facts.clone(),
        ToolRegistry::new(),
        Arc::new(Approval {
            calls: AtomicUsize::new(0),
            approved: true,
        }),
        Arc::new(HelperHook {
            calls: AtomicUsize::new(0),
        }),
    );
    let task = tokio::spawn(async move { ops.complete_model(request()).await });
    while provider.calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    cancellation.cancel();
    assert!(task.await.unwrap().is_err());
    assert_eq!(facts.errors.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn lifecycle_notifications_cannot_start_child_work() {
    let facts = Arc::new(Facts::default());
    let provider = Arc::new(Provider {
        calls: AtomicUsize::new(0),
        wait: false,
    });
    let (chain, _ops, _root, tools) = bind(
        ExecutionScope::fresh(CancellationToken::new()),
        provider.clone(),
        facts.clone(),
        ToolRegistry::new(),
        Arc::new(Approval {
            calls: AtomicUsize::new(0),
            approved: true,
        }),
        Arc::new(HelperHook {
            calls: AtomicUsize::new(0),
        }),
    );
    chain
        .apply(HookInput {
            event: HookEvent::TurnSettled {
                status: HookTurnStatus::Success,
                output: None,
                error: None,
            },
            attribution: tools.binding().attribution(),
            cwd: "/tmp".into(),
            conversation: None,
        })
        .await
        .unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert!(facts.tools.lock().unwrap().is_empty());
    assert_eq!(facts.traces.lock().unwrap().len(), 1);
}
