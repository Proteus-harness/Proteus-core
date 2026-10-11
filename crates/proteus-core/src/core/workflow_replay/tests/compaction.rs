use crate::{
    contracts::{
        ContextPreparationInput, ContextPreparationOutput, HookEvent, HookInput, HookResponse,
        HookStep, HookStepOutcome, HookTrace, WorkflowHistoryCheckpoint, WorkflowHistoryUpdate,
    },
    domain::HistoryCompactionReport,
};

use super::*;

const COMPACTION_WORKFLOW_ID: &str = "replay.compaction_probe";
const COMPACTION_REASON: &str = "workflow_replay_test";

#[test]
fn compaction_comparison_preserves_opaque_metadata_even_with_known_field_names() {
    let mut expected = HistoryCompactionReport::unchanged(3, Some("test".to_owned()));
    expected.changed = true;
    expected.output_messages = 2;
    expected.original_token_estimate = Some(300);
    expected.output_token_estimate = Some(30);
    let mut replay = expected.clone();
    assert!(super::super::normalize::changed_compactions_equal(
        std::slice::from_ref(&replay),
        std::slice::from_ref(&expected),
    ));
    replay.metadata = json!({
        "input_messages": 3,
        "output_messages": 2,
        "original_token_estimate": 300,
        "output_token_estimate": 30
    });
    assert!(!super::super::normalize::changed_compactions_equal(
        std::slice::from_ref(&replay),
        std::slice::from_ref(&expected),
    ));

    replay.metadata = json!({ "module_signal": "changed" });
    assert!(!super::super::normalize::changed_compactions_equal(
        std::slice::from_ref(&replay),
        std::slice::from_ref(&expected),
    ));
}

#[derive(Clone, Copy)]
struct CompactionProbeWorkflow {
    mode: CompactionProbeMode,
}

#[derive(Clone, Copy)]
enum CompactionProbeMode {
    Match,
    DivergeReport,
    InvalidHistory,
}

#[async_trait]
impl Workflow for CompactionProbeWorkflow {
    async fn run(
        &self,
        task: AgentTask,
        history: Vec<CanonicalMessage>,
        ctx: crate::contracts::WorkflowInvocationContext,
    ) -> anyhow::Result<WorkflowOutput> {
        let ctx = ctx.into_agent()?;
        let compaction_input =
            replay_compaction_input(&task, &history, ctx.model_ref.as_ref().unwrap());
        let preparation = ctx
            .context_preparation
            .prepare(compaction_input.clone())
            .await?;
        let compacted = preparation.output;
        let mut report =
            HistoryCompactionReport::from_context_preparation_output(&compaction_input, &compacted);
        ctx.history_recorder
            .checkpoint(WorkflowHistoryCheckpoint {
                history: WorkflowHistoryUpdate::new(vec![])
                    .with_history_replacement(compacted.messages.clone())
                    .with_compactions(vec![report.clone()]),
                tool_results: vec![],
                preparation_target: preparation.target,
            })
            .await?;
        if matches!(self.mode, CompactionProbeMode::DivergeReport) {
            report.summary_source = Some("changed_implementation".to_owned());
        }
        let request = CanonicalModelRequest::new(
            ctx.model_ref.clone().expect("model selection"),
            compacted.messages.clone(),
        )
        .with_tools(vec![probe_tool_spec()]);
        let response = ctx.execution.require_model()?.complete(request).await?;
        Ok(
            WorkflowOutput::new(AgentOutput::text("compacted"), response.messages)
                .with_history_replacement(compacted.messages)
                .with_compactions(
                    if matches!(self.mode, CompactionProbeMode::InvalidHistory) {
                        Vec::new()
                    } else {
                        vec![report]
                    },
                ),
        )
    }
}

fn replay_compaction_input(
    _task: &AgentTask,
    history: &[CanonicalMessage],
    model_ref: &ModelRef,
) -> ContextPreparationInput {
    ContextPreparationInput::new(
        history.to_vec(),
        Some(
            proteus_contracts::model_standard::CanonicalModelRequest::new(
                model_ref.clone(),
                history.to_vec(),
            ),
        ),
    )
    .with_reason(COMPACTION_REASON)
    .with_token_estimate(Some(400))
}

fn compaction_catalog(mode: CompactionProbeMode) -> ModuleCatalog {
    let mut catalog = ModuleCatalog::new();
    catalog.register_test_workflow(
        COMPACTION_WORKFLOW_ID,
        Arc::new(CompactionProbeWorkflow { mode }),
    );
    catalog.register_test_policy(POLICY_ID, Arc::new(ReplayAllowAll));
    catalog
}

async fn compacted_journal() -> TestJournal {
    compacted_journal_with_metadata(json!({
        "module_signal": "recorded",
        "input_messages": 999,
        "trigger_tokens": "private-diagnostic"
    }))
    .await
}

async fn compacted_journal_with_metadata(metadata: serde_json::Value) -> TestJournal {
    let config_dir = tempfile::tempdir().expect("config dir");
    let workspace = tempfile::tempdir().expect("workspace");
    let session_id = new_session_id();
    let thread_id = new_thread_id();
    let turn_id = new_turn_id();
    let execution_id = new_execution_id();
    let store =
        SessionStore::new(config_dir.path(), workspace.path(), session_id).expect("session store");
    let task = AgentTask::new(
        "run compaction replay probe",
        workspace.path().to_path_buf(),
    );
    let user = CanonicalMessage::text(MessageRole::User, task.text.clone());
    let spec = probe_tool_spec();
    let mut recorded_snapshot = snapshot(&spec);
    recorded_snapshot.modules.workflow = Some(COMPACTION_WORKFLOW_ID.to_owned());
    recorded_snapshot.modules.hooks = vec!["replay.preparation".into()];

    store
        .append_execution_journal_entry(
            ExecutionAttribution::for_turn(execution_id, session_id, thread_id, turn_id),
            JournalEntry::TurnOpened(TurnOpened {
                intent: None,
                task: task.clone(),
                base_history_revision: 0,
                module_epoch: 5,
                config_snapshot: serde_json::to_value(recorded_snapshot).expect("snapshot value"),
            }),
        )
        .await
        .expect("turn opened");
    store
        .append_history(thread_id, Some(turn_id), std::slice::from_ref(&user))
        .await
        .expect("user history");

    let attribution = ExecutionAttribution::for_turn(execution_id, session_id, thread_id, turn_id);
    append_preparation_hook(
        &store,
        attribution,
        &task,
        HookEvent::TurnStarted {
            task: task.clone(),
            history: vec![user.clone()],
        },
        HookResponse::Continue,
    )
    .await;
    let compaction_input = replay_compaction_input(
        &task,
        std::slice::from_ref(&user),
        &ModelRef::new("missing-provider", "offline-model"),
    );
    let summary = CanonicalMessage::text(MessageRole::User, "recorded compacted summary");
    let compacted_user = CanonicalMessage::from_parts(
        MessageRole::User,
        vec![crate::model_standard::CanonicalPart::new(
            crate::model_standard::PartProvenance::Compactor,
            crate::model_standard::PartScope::Conversation,
            ContentPart::Text {
                text: "compacted user input".to_owned(),
            },
        )],
    );
    let replacement = crate::domain::CompactionUserMessageReplacement {
        source_message_id: user.id,
        replacement_message_id: compacted_user.id,
    };
    let compacted_messages = vec![summary, compacted_user];
    let mut compaction_output = ContextPreparationOutput::changed(
        compacted_messages.clone(),
        Some("recorded compacted summary".to_owned()),
    );
    compaction_output.token_estimate = Some(40);
    compaction_output
        .user_message_replacements
        .push(replacement);
    compaction_output.original_token_estimate = Some(400);
    compaction_output.trigger_tokens = Some(100);
    compaction_output.summary_source = Some("test_fixture".to_owned());
    compaction_output.metadata = metadata;
    let report = HistoryCompactionReport::from_context_preparation_output(
        &compaction_input,
        &compaction_output,
    );
    let response = CanonicalModelResponse::new(
        CanonicalMessage::text(MessageRole::Assistant, "compacted response"),
        Vec::new(),
        FinishReason::Stop,
    );
    append_preparation_hook(
        &store,
        attribution,
        &task,
        HookEvent::BeforeContext {
            input: compaction_input.clone(),
            output: ContextPreparationOutput::unchanged(vec![user.clone()]),
            target: Some(crate::contracts::ContextPreparationTarget {
                execution_id,
                revision: 1,
            }),
        },
        HookResponse::ContextPreparation {
            output: compaction_output.clone(),
        },
    )
    .await;
    store
        .checkpoint_history(
            thread_id,
            turn_id,
            &compacted_messages,
            Some(report.clone()),
            vec![],
        )
        .await
        .unwrap();
    let request = recorded_request(
        session_id,
        thread_id,
        turn_id,
        compacted_messages.clone(),
        spec.clone(),
    );
    append_preparation_hook(
        &store,
        attribution,
        &task,
        HookEvent::BeforeModel {
            origin: crate::contracts::ModelCallOrigin::Direct,
            request,
        },
        HookResponse::Continue,
    )
    .await;
    append_exchange(
        &store,
        execution_id,
        thread_id,
        turn_id,
        recorded_request(
            session_id,
            thread_id,
            turn_id,
            compacted_messages.clone(),
            spec,
        ),
        response.clone(),
    )
    .await;

    let mut final_history = compacted_messages;
    final_history.extend(response.messages);
    store
        .checkpoint_history(thread_id, turn_id, &final_history, None, vec![])
        .await
        .expect("compacted history");
    append_preparation_hook(
        &store,
        attribution,
        &task,
        HookEvent::BeforeStop {
            task: task.clone(),
            history: final_history.clone(),
            output: AgentOutput::text("compacted"),
            attempt: 0,
            continuation: None,
        },
        HookResponse::Continue,
    )
    .await;
    append_preparation_hook(
        &store,
        attribution,
        &task,
        HookEvent::TurnSettled {
            status: crate::contracts::HookTurnStatus::Success,
            output: Some(AgentOutput::text("compacted")),
            error: None,
        },
        HookResponse::Continue,
    )
    .await;
    store
        .append_journal_entry(
            thread_id,
            Some(turn_id),
            JournalEntry::TurnSettled(TurnSettled {
                status: TurnSettlementStatus::Success,
                output: Some(AgentOutput::text("compacted")),
                error: None,
            }),
        )
        .await
        .expect("turn settled");

    TestJournal {
        _config_dir: config_dir,
        _workspace: workspace,
        store,
        thread_id,
        turn_id,
    }
}

async fn append_preparation_hook(
    store: &SessionStore,
    attribution: ExecutionAttribution,
    task: &AgentTask,
    event: HookEvent,
    response: HookResponse,
) {
    let output = crate::contracts::apply_hook_response(&event, &response).unwrap();
    store
        .append_execution_journal_entry(
            attribution,
            JournalEntry::HookInvoked(HookTrace {
                owner: crate::contracts::OperationOwner::Workflow,
                input: HookInput {
                    event,
                    attribution,
                    cwd: task.cwd.clone(),
                    conversation: None,
                },
                steps: vec![HookStep {
                    invocation_id: uuid::Uuid::new_v4(),
                    module_id: "replay.preparation".into(),
                    outcome: HookStepOutcome::Accepted { response },
                }],
                output: Some(output),
            }),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn changed_compaction_replays_the_declared_user_message_replacement() {
    let journal = compacted_journal().await;
    let before = std::fs::read(journal.store.journal_path()).expect("journal before");

    let report = replay_workflow(
        journal.store.session_dir(),
        &AppConfig::default(),
        &compaction_catalog(CompactionProbeMode::Match),
        WorkflowReplayOptions::default(),
    )
    .await
    .expect("changed compaction replay");

    let after = std::fs::read(journal.store.journal_path()).expect("journal after");
    assert!(report.comparison.matched, "{:?}", report.comparison.issues);
    assert_eq!(report.comparison.history_equal, Some(true));
    assert_eq!(report.comparison.output_equal, Some(true));
    assert_eq!(report.model_exchanges.recorded, 1);
    assert_eq!(report.model_exchanges.replayed, 1);
    assert_eq!(before, after);
}

#[tokio::test]
async fn changed_compaction_replay_preserves_non_object_metadata() {
    for metadata in [
        json!(["module-defined", 42]),
        json!("diagnostic"),
        json!({}),
        json!(null),
    ] {
        let journal = compacted_journal_with_metadata(metadata).await;
        let report = replay_workflow(
            journal.store.session_dir(),
            &AppConfig::default(),
            &compaction_catalog(CompactionProbeMode::Match),
            WorkflowReplayOptions::default(),
        )
        .await
        .expect("opaque metadata replay");
        assert!(report.comparison.matched, "{:?}", report.comparison.issues);
        assert!(report.source_journal_unchanged);
    }
}

#[tokio::test]
async fn changed_compaction_report_divergence_is_not_hidden_by_equal_history() {
    let journal = compacted_journal().await;

    let report = replay_workflow(
        journal.store.session_dir(),
        &AppConfig::default(),
        &compaction_catalog(CompactionProbeMode::DivergeReport),
        WorkflowReplayOptions::default(),
    )
    .await
    .expect("compaction report divergence");

    assert!(!report.comparison.matched);
    assert_eq!(report.comparison.history_equal, Some(true));
    assert_eq!(report.comparison.output_equal, Some(true));
    assert!(report.comparison.issues.iter().any(|issue| {
        issue.contains("changed compaction reports differ from the canonical journal")
    }));
    assert!(report.source_journal_unchanged);
}

#[tokio::test]
async fn replay_uses_runtime_history_validation_for_workflow_output() {
    let journal = compacted_journal().await;

    let report = replay_workflow(
        journal.store.session_dir(),
        &AppConfig::default(),
        &compaction_catalog(CompactionProbeMode::InvalidHistory),
        WorkflowReplayOptions::default(),
    )
    .await
    .expect("invalid history divergence report");

    assert!(!report.comparison.matched);
    assert_eq!(report.replay.status, TurnSettlementStatus::Error);
    assert!(
        report.replay.error.as_deref().is_some_and(|error| {
            error.contains("history replacement without changed compaction")
        })
    );
    assert!(report.source_journal_unchanged);
}
