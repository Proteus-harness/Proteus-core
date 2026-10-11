//! Selection is an explicit journal boundary, including hidden, unused tools
//! and errors which have no following model request.
use super::*;
use crate::contracts::{
    ApprovalRequest, ApprovalResponse, ApprovalTransport, ExecutionPermissionGrants,
    ExecutionScope, HookResponse, HookStep, HookStepOutcome, HookTrace, OperationOwner, Tool,
    ToolContext, ToolRegistry, ToolSelectionInput, ToolSelectionOutput, ToolSelectionRequest,
};
const FAILURE: &str = "selection workflow ended before sampling";
struct SelectionWorkflow {
    changed_phase: bool,
    selection_fails: bool,
    availability: Option<bool>,
}
#[async_trait]
impl Workflow for SelectionWorkflow {
    async fn run(
        &self,
        task: AgentTask,
        _: Vec<CanonicalMessage>,
        ctx: crate::contracts::WorkflowInvocationContext,
    ) -> anyhow::Result<WorkflowOutput> {
        let host = crate::core::workflow_host::WorkflowHostRuntime::new(ctx, task.cwd.clone());
        let request = ToolSelectionRequest::new(task)
            .with_max_tools(1)
            .with_phase(if self.changed_phase {
                "changed"
            } else {
                "plan"
            });
        let result = host.select_tools(request).await;
        if self.selection_fails {
            assert!(result.is_err());
        } else if !self.changed_phase {
            let output = result?;
            assert_eq!(output.tools.len(), 1);
            assert_eq!(
                output.tools[0].name,
                if self.availability == Some(false) {
                    "z_hidden_a"
                } else {
                    "write_marker"
                }
            );
            assert_eq!(output.metadata, json!({"hidden":2}));
        }
        anyhow::bail!(FAILURE)
    }
}
async fn source(selection_fails: bool, availability: Option<bool>) -> TestJournal {
    let config_dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let session_id = new_session_id();
    let thread_id = new_thread_id();
    let turn_id = new_turn_id();
    let execution_id = new_execution_id();
    let attribution = ExecutionAttribution::for_turn(execution_id, session_id, thread_id, turn_id);
    let store = SessionStore::new(config_dir.path(), workspace.path(), session_id).unwrap();
    let task = AgentTask::new("select tools only", workspace.path().into());
    let user = CanonicalMessage::text(MessageRole::User, &task.text);
    let spec = probe_tool_spec();
    let mut snapshot = snapshot(&spec);
    snapshot.modules.hooks = vec!["select.fixture".into()];
    snapshot.approval_available = availability.unwrap_or(true);
    store
        .append_execution_journal_entry(
            attribution,
            JournalEntry::TurnOpened(TurnOpened {
                intent: None,
                task: task.clone(),
                base_history_revision: 0,
                module_epoch: 1,
                config_snapshot: serde_json::to_value(snapshot).unwrap(),
            }),
        )
        .await
        .unwrap();
    store
        .append_history(thread_id, Some(turn_id), std::slice::from_ref(&user))
        .await
        .unwrap();
    let hook_input = |event| HookInput {
        event,
        attribution,
        cwd: task.cwd.clone(),
        conversation: None,
    };
    let trace = |event: HookEvent, response: HookResponse| HookTrace {
        owner: OperationOwner::Workflow,
        input: hook_input(event.clone()),
        steps: vec![HookStep {
            invocation_id: uuid::Uuid::new_v4(),
            module_id: "select.fixture".into(),
            outcome: HookStepOutcome::Accepted {
                response: response.clone(),
            },
        }],
        output: Some(crate::contracts::apply_hook_response(&event, &response).unwrap()),
    };
    store
        .append_execution_journal_entry(
            attribution,
            JournalEntry::HookInvoked(trace(
                HookEvent::TurnStarted {
                    task: task.clone(),
                    history: vec![user],
                },
                HookResponse::Continue,
            )),
        )
        .await
        .unwrap();
    // These tools were omitted from the assembly snapshot, never exposed in a
    // model request and never invoked. The selection input is their evidence.
    let all_candidates = vec![
        spec.clone(),
        ToolSpec::new(
            "z_hidden_a",
            "unused",
            json!({"type":"object"}),
            ToolSafety::ReadOnly,
        ),
        ToolSpec::new(
            "z_hidden_b",
            "unused",
            json!({"type":"object"}),
            ToolSafety::ReadOnly,
        ),
    ];
    let candidates = if let Some(available) = availability {
        let mut tools = ToolRegistry::new();
        for spec in all_candidates {
            tools.register(VisibilityTool(spec)).unwrap();
        }
        let approval: Arc<dyn ApprovalTransport> = if available {
            Arc::new(InteractiveApproval)
        } else {
            Arc::new(crate::core::HeadlessApprovalTransport)
        };
        let authority = crate::core::BoundTools::new(
            tools,
            Arc::new(AskForWrites),
            approval,
            Arc::new(ExecutionPermissionGrants::default()),
            crate::core::ToolExecutionBinding::detached(ExecutionScope::fresh(Default::default())),
        );
        let visible = authority.visible_specs(&task.cwd);
        assert_eq!(
            visible.iter().any(|tool| tool.name == "write_marker"),
            available
        );
        visible
    } else {
        all_candidates
    };
    let selected = candidates.iter().take(1).cloned().collect();
    let event = HookEvent::BeforeToolSelection {
        input: ToolSelectionInput::new(
            ToolSelectionRequest::new(task.clone())
                .with_max_tools(1)
                .with_phase("plan"),
            candidates.clone(),
        ),
        output: ToolSelectionOutput::new(candidates),
    };
    let selection = if selection_fails {
        HookTrace {
            owner: OperationOwner::Workflow,
            input: hook_input(event),
            steps: vec![HookStep {
                invocation_id: uuid::Uuid::new_v4(),
                module_id: "select.fixture".into(),
                outcome: HookStepOutcome::Failed {
                    message: "selection failed".into(),
                    model_failure: None,
                },
            }],
            output: None,
        }
    } else {
        let mut output = ToolSelectionOutput::new(selected);
        output.metadata = json!({"hidden":2});
        trace(event, HookResponse::ToolSelection { output })
    };
    store
        .append_execution_journal_entry(attribution, JournalEntry::HookInvoked(selection))
        .await
        .unwrap();
    store
        .append_execution_journal_entry(
            attribution,
            JournalEntry::HookInvoked(trace(
                HookEvent::TurnSettled {
                    status: HookTurnStatus::Error,
                    output: None,
                    error: Some(FAILURE.into()),
                },
                HookResponse::Continue,
            )),
        )
        .await
        .unwrap();
    store
        .append_journal_entry(
            thread_id,
            Some(turn_id),
            JournalEntry::TurnSettled(TurnSettled {
                status: TurnSettlementStatus::Error,
                output: None,
                error: Some(FAILURE.into()),
            }),
        )
        .await
        .unwrap();
    TestJournal {
        _config_dir: config_dir,
        _workspace: workspace,
        store,
        thread_id,
        turn_id,
    }
}
async fn replay(
    selection_fails: bool,
    changed_phase: bool,
    availability: Option<bool>,
) -> WorkflowReplayReport {
    let source = source(selection_fails, availability).await;
    let mut catalog = ModuleCatalog::new();
    catalog.register_test_workflow(
        WORKFLOW_ID,
        Arc::new(SelectionWorkflow {
            selection_fails,
            changed_phase,
            availability,
        }),
    );
    let policy: Arc<dyn ApprovalPolicy> = if availability.is_some() {
        Arc::new(AskForWrites)
    } else {
        Arc::new(ReplayAllowAll)
    };
    catalog.register_test_policy(POLICY_ID, policy);
    replay_workflow(
        source.store.session_dir(),
        &AppConfig::default(),
        &catalog,
        WorkflowReplayOptions::default(),
    )
    .await
    .unwrap()
}
#[tokio::test]
async fn hidden_uncalled_tools_and_selection_metadata_replay_without_sampling() {
    let report = replay(false, false, None).await;
    assert!(report.comparison.matched, "{:?}", report.comparison.issues);
    assert_eq!(report.model_exchanges.recorded, 0);
    assert!(report.source_journal_unchanged);
}
#[tokio::test]
async fn selection_failure_replays_without_a_following_model_request() {
    let report = replay(true, false, None).await;
    assert!(report.comparison.matched, "{:?}", report.comparison.issues);
    assert!(report.source_journal_unchanged);
}
#[tokio::test]
async fn changed_selection_phase_diverges_even_when_workflow_swallows_error() {
    let report = replay(false, true, None).await;
    assert!(!report.comparison.matched);
    assert!(
        report
            .comparison
            .issues
            .iter()
            .any(|issue| issue.contains("before_tool_selection input differs")),
        "{:?}",
        report.comparison.issues
    );
}

struct VisibilityTool(ToolSpec);
#[async_trait]
impl Tool for VisibilityTool {
    fn spec(&self) -> ToolSpec {
        self.0.clone()
    }
    async fn invoke(&self, _: &ToolCall, _: ToolContext) -> anyhow::Result<ToolResult> {
        panic!("visibility must not invoke tools")
    }
}
struct AskForWrites;
impl ApprovalPolicy for AskForWrites {
    fn evaluate(&self, _: &ToolCall, _: &PolicyContext) -> PolicyDecision {
        PolicyDecision::Allow
    }
    fn evaluate_visibility(&self, ctx: &PolicyVisibilityContext) -> PolicyDecision {
        if ctx.tool_spec.name == "write_marker" {
            PolicyDecision::Ask {
                reason: "approval for writes".into(),
            }
        } else {
            PolicyDecision::Allow
        }
    }
}
struct InteractiveApproval;
#[async_trait]
impl ApprovalTransport for InteractiveApproval {
    fn can_request_approval(&self) -> bool {
        true
    }
    async fn request_approval(&self, _: ApprovalRequest) -> anyhow::Result<ApprovalResponse> {
        panic!("selection must not request approval")
    }
}
#[tokio::test]
async fn interactive_ask_selection_replays_exact_candidates() {
    let report = replay(false, false, Some(true)).await;
    assert!(report.comparison.matched, "{:?}", report.comparison.issues);
}
#[tokio::test]
async fn noninteractive_ask_selection_replays_recorded_transport_availability() {
    let report = replay(false, false, Some(false)).await;
    assert!(report.comparison.matched, "{:?}", report.comparison.issues);
    assert!(report.source_journal_unchanged);
}
