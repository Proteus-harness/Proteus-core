//! Real store capture/checkpoint semantics, without a provider or process module.
use super::{incremental::*, *};
use crate::{
    contracts::{ToolExecutionRecorder, WorkflowToolResultBinding},
    core::SessionToolExecutionRecorder,
    domain::{ToolCall, ToolCallResolution, ToolResult},
    model_standard::ContentPart,
};

#[tokio::test]
async fn full_workflow_checkpoints_and_tool_result_ack_rebase_use_incremental_projection() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "prior settled message").await;
    let prior = project(&store);
    let mut history = store.load_messages().unwrap();
    let thread = new_thread_id();
    let turn = new_turn_id();
    let attribution =
        ExecutionAttribution::for_turn(new_execution_id(), store.session_id(), thread, turn);
    store
        .append_execution_journal_entry(
            attribution,
            JournalEntry::TurnOpened(TurnOpened {
                intent: None,
                task: AgentTask::new("normal checkpoint workflow", workspace.path().into()),
                base_history_revision: prior.revision,
                module_epoch: 0,
                config_snapshot: serde_json::json!({}),
            }),
        )
        .await
        .unwrap();
    let user = CanonicalMessage::text(MessageRole::User, "new user turn");
    store
        .append_history(thread, Some(turn), std::slice::from_ref(&user))
        .await
        .unwrap();
    history.push(user);
    store
        .checkpoint_history(thread, turn, &history, None, vec![])
        .await
        .unwrap();

    let calls = [
        ToolCall::new("checkpoint-first", "probe", serde_json::json!({"n": 1})),
        ToolCall::new("checkpoint-second", "probe", serde_json::json!({"n": 2})),
    ];
    let bindings: Vec<_> = calls
        .iter()
        .cloned()
        .map(WorkflowToolResultBinding::new)
        .collect();
    history.push(CanonicalMessage::new(
        MessageRole::Assistant,
        calls
            .iter()
            .cloned()
            .map(|call| ContentPart::ToolCall { call })
            .collect(),
    ));
    store
        .checkpoint_history(thread, turn, &history, None, bindings.clone())
        .await
        .unwrap();
    let recorder = SessionToolExecutionRecorder::new(store.clone());
    for call in &calls {
        recorder
            .tool_call_requested(attribution, OperationOwner::Workflow, call)
            .await
            .unwrap();
        recorder
            .tool_call_resolved(attribution, call, &ToolCallResolution::Allowed)
            .await
            .unwrap();
    }
    // Complete out of order, then ACK one result while the other remains a
    // durable captured result. The store rebases that result behind the ACK.
    let first = ToolResult::ok(calls[0].id.clone(), "first completed later");
    let second = ToolResult::ok(calls[1].id.clone(), "second completed first");
    recorder
        .tool_result_recorded(attribution, &second)
        .await
        .unwrap();
    recorder
        .tool_result_recorded(attribution, &first)
        .await
        .unwrap();
    history.push(bindings[0].message(first));
    history.push(CanonicalMessage::text(
        MessageRole::Assistant,
        "acknowledged the first tool result",
    ));
    store
        .checkpoint_history(thread, turn, &history, None, vec![bindings[1].clone()])
        .await
        .unwrap();
    history.push(bindings[1].message(second));
    assert_eq!(store.load_messages().unwrap(), history);
    history.push(CanonicalMessage::text(MessageRole::Assistant, "done"));
    store
        .checkpoint_history(thread, turn, &history, None, vec![])
        .await
        .unwrap();
    store
        .append_journal_entry(
            thread,
            Some(turn),
            JournalEntry::TurnSettled(TurnSettled {
                status: TurnSettlementStatus::Success,
                output: None,
                error: None,
            }),
        )
        .await
        .unwrap();
    let expected = oracle(&store);
    let journal = fs::read(store.journal_path()).unwrap();
    let view = store
        .project_read_view(
            |_| panic!("ordinary full checkpoints must not defeat incremental projection"),
            crate::app_server::journal_transcript_append_from_source,
        )
        .unwrap();
    assert_display_view(&view, &expected);
    assert_same_view(&store.fresh_read_model().unwrap(), &expected);
    let base = view.cursor.unwrap().base.unwrap();
    assert_eq!(base.revision, prior.cursor.unwrap().revision);
    assert_eq!(base.common_prefix, 1);
    assert_eq!(fs::read(store.journal_path()).unwrap(), journal);
}
