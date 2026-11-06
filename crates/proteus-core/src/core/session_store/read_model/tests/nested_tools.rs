//! UI projection may retain nested tools without exposing auxiliary model text
//! or adding their calls/results to model-visible canonical history.
use super::{incremental::*, *};
use crate::{
    contracts::{ToolExecutionRecorder, ToolTerminalRef},
    core::{SessionToolExecutionRecorder, ToolResultRecorded},
    domain::{ToolCall, ToolCallResolution, ToolResult},
    model_standard::ContentPart,
};
use serde_json::json;

async fn nested_turn(
    store: &SessionStore,
    workspace: &Path,
    canceled: bool,
) -> Vec<CanonicalMessage> {
    let attribution = ExecutionAttribution::for_turn(
        new_execution_id(),
        store.session_id(),
        new_thread_id(),
        new_turn_id(),
    );
    let agent = attribution.agent.unwrap();
    store
        .append_execution_journal_entry(
            attribution,
            JournalEntry::TurnOpened(TurnOpened {
                intent: None,
                task: AgentTask::new("nested terminal", workspace.into()),
                base_history_revision: store.load_projection().unwrap().history_revision,
                module_epoch: 0,
                config_snapshot: json!({}),
            }),
        )
        .await
        .unwrap();
    let recorder = SessionToolExecutionRecorder::new(store.clone());
    let parent = ToolCall::new("parent", "proxy", json!({}));
    recorder
        .tool_call_requested(attribution, OperationOwner::Workflow, &parent)
        .await
        .unwrap();
    recorder
        .tool_call_resolved(attribution, &parent, &ToolCallResolution::Allowed)
        .await
        .unwrap();
    let mut returned = vec![CanonicalMessage::new(
        MessageRole::Assistant,
        vec![ContentPart::ToolCall {
            call: parent.clone(),
        }],
    )];
    for (id, owner) in [
        (
            "child",
            OperationOwner::Tool {
                call_id: parent.id.clone(),
            },
        ),
        (
            "hook-child",
            OperationOwner::Hook {
                invocation_id: uuid::Uuid::new_v4(),
            },
        ),
    ] {
        let call = ToolCall::new(id, "shell", json!({"tty":true}));
        if id == "child" {
            let exchange_id = new_exchange_id();
            store
                .append_execution_journal_entry(
                    attribution,
                    JournalEntry::ModelRequestRecorded(ModelRequestRecorded {
                        exchange_id,
                        origin: ModelCallOrigin::Auxiliary,
                        owner: owner.clone(),
                        request: CanonicalModelRequest::new(
                            ModelRef::new("fixture", "auxiliary"),
                            vec![],
                        ),
                    }),
                )
                .await
                .unwrap();
            store
                .append_execution_journal_entry(
                    attribution,
                    JournalEntry::ModelResponseRecorded(ModelResponseRecorded {
                        exchange_id,
                        outcome: ModelResponseOutcome::Response {
                            response: CanonicalModelResponse::new(
                                CanonicalMessage::new(
                                    MessageRole::Assistant,
                                    vec![
                                        ContentPart::Text {
                                            text: "PRIVATE auxiliary model text".into(),
                                        },
                                        ContentPart::ToolCall { call: call.clone() },
                                    ],
                                ),
                                vec![call.clone()],
                                FinishReason::ToolCalls,
                            ),
                        },
                    }),
                )
                .await
                .unwrap();
        }
        recorder
            .tool_call_requested(attribution, owner, &call)
            .await
            .unwrap();
        recorder
            .tool_call_resolved(attribution, &call, &ToolCallResolution::Allowed)
            .await
            .unwrap();
        if !canceled {
            let result = ToolResult::ok(id.into(), format!("ordinary output from {id}"))
                .with_metadata(json!({"tool_terminal":ToolTerminalRef {
                    id: uuid::Uuid::new_v4(), title: id.into(), tty: true,
                }}));
            // Both facts are present when after_tool hooks run; only the final
            // result should be rendered, not a duplicate effect row.
            store
                .append_execution_journal_entry(
                    attribution,
                    JournalEntry::ToolEffectRecorded(ToolResultRecorded {
                        result: result.clone(),
                    }),
                )
                .await
                .unwrap();
            recorder
                .tool_result_recorded(attribution, &result)
                .await
                .unwrap();
        }
    }
    if !canceled {
        let result = ToolResult::ok(parent.id.clone(), "proxy finished");
        recorder
            .tool_result_recorded(attribution, &result)
            .await
            .unwrap();
        returned.extend([
            CanonicalMessage::new(MessageRole::Tool, vec![ContentPart::ToolResult { result }]),
            CanonicalMessage::text(MessageRole::Assistant, "root returned only its own parts"),
        ]);
        store
            .append_history(agent.thread_id, Some(agent.turn_id), &returned)
            .await
            .unwrap();
    }
    store
        .append_journal_entry(
            agent.thread_id,
            Some(agent.turn_id),
            JournalEntry::TurnSettled(TurnSettled {
                status: if canceled {
                    TurnSettlementStatus::Canceled
                } else {
                    TurnSettlementStatus::Success
                },
                output: None,
                error: None,
            }),
        )
        .await
        .unwrap();
    returned
}

fn assert_nested_rows(rows: &[AppTranscriptMessage]) {
    assert!(rows.iter().all(|row| !row.text.contains("PRIVATE")));
    for id in ["parent", "child", "hook-child"] {
        let tools: Vec<_> = rows
            .iter()
            .filter_map(|row| row.tool.as_ref())
            .filter(|tool| tool.call_id == id)
            .collect();
        assert_eq!(
            tools.len(),
            1,
            "canonical duplicates and effect/final facts deduplicate for {id}"
        );
        let tool = tools[0];
        assert_eq!(tool.status, "done");
        if id == "parent" {
            assert!(tool.terminal.is_none());
            assert_eq!(tool.result.as_deref(), Some("proxy finished"));
        } else {
            assert_eq!(
                tool.result.as_deref(),
                Some(format!("ordinary output from {id}").as_str())
            );
            assert_eq!(tool.terminal.as_ref().unwrap().title, id);
        }
    }
}

#[tokio::test]
async fn nested_terminal_rows_survive_warm_cold_paging_and_old_cache_rebuild_without_history_injection()
 {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "prompt").await;
    // Establish an older certified prefix to exercise the append projection.
    project(&store);
    let returned = nested_turn(&store, workspace.path(), false).await;
    let canonical = store.load_messages().unwrap();
    assert_eq!(
        &canonical[1..],
        returned.as_slice(),
        "only the workflow's declared parts enter canonical history"
    );
    project(&store);
    let warm = store.fresh_read_model().unwrap();
    assert_nested_rows(&warm.transcript);
    let collected = journal_transcript_messages(store.load_records().unwrap(), None);
    assert_eq!(
        serde_json::to_value(&collected).unwrap(),
        serde_json::to_value(&warm.transcript).unwrap()
    );

    // Push both child rows outside the last fifty so recovery has to page.
    let later: Vec<_> = (0..60)
        .map(|index| CanonicalMessage::text(MessageRole::User, format!("later {index}")))
        .collect();
    store
        .append_history(new_thread_id(), None, &later)
        .await
        .unwrap();
    project(&store);
    let warm = store.fresh_read_model().unwrap();
    let tail = store.saved_transcript_page(None, None).unwrap();
    assert_eq!(tail.messages.len(), 50);
    assert!(tail.messages.iter().all(|row| row.tool.is_none()));
    let older = store
        .saved_transcript_page(Some(tail.start), Some(&tail.revision))
        .unwrap();
    assert_nested_rows(&older.messages);

    let journal = fs::read(store.journal_path()).unwrap();
    let fingerprint =
        crate::core::session_store::read_cache::SourceFingerprint::read(store.session_dir(), 8192)
            .unwrap();
    // Previous releases hashed only the source. A format bump must also
    // change the paging revision, not just reject the old SQLite file.
    let old_revision = ring::digest::digest(
        &ring::digest::SHA256,
        &serde_json::to_vec(&fingerprint).unwrap(),
    )
    .as_ref()
    .iter()
    .map(|byte| format!("{byte:02x}"))
    .collect::<String>();
    assert_ne!(old_revision, tail.revision);
    let reopened = SessionStore::open(store.session_dir().into()).unwrap();
    project(&reopened);
    assert_same_view(&reopened.fresh_read_model().unwrap(), &warm);
    assert_eq!(
        serde_json::to_value(
            reopened
                .saved_transcript_page(Some(tail.start), Some(&tail.revision))
                .unwrap()
                .messages
        )
        .unwrap(),
        serde_json::to_value(&older.messages).unwrap()
    );

    // A pre-fix view has the same source fingerprint and valid checksums.
    // Its format must still be rejected, including the paging fast path.
    let db = rusqlite::Connection::open(store.session_dir().join(".read-view.sqlite")).unwrap();
    let bytes: Vec<u8> = db
        .query_row(
            "SELECT payload FROM records WHERE section='source'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut source: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    source["format"] = json!(3);
    let bytes = serde_json::to_vec(&source).unwrap();
    let checksum = ring::digest::digest(&ring::digest::SHA256, &bytes);
    db.execute(
        "UPDATE records SET payload=?1,checksum=?2 WHERE section='source'",
        rusqlite::params![bytes, checksum.as_ref()],
    )
    .unwrap();
    drop(db);
    assert!(reopened.saved_transcript_page(None, None).is_none());
    assert!(reopened.fresh_read_model().is_none());
    project(&reopened);
    assert_same_view(&reopened.fresh_read_model().unwrap(), &warm);
    assert!(
        reopened
            .saved_transcript_page(Some(tail.start), Some(&old_revision))
            .is_none()
    );
    assert_eq!(fs::read(store.journal_path()).unwrap(), journal);
}

#[tokio::test]
async fn canceled_nested_calls_recover_as_interrupted_without_invented_output_or_terminal_ref() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "prompt").await;
    let canonical = store.load_messages().unwrap();
    nested_turn(&store, workspace.path(), true).await;
    project(&store);
    let model = store.fresh_read_model().unwrap();
    for id in ["parent", "child", "hook-child"] {
        let tool = model
            .transcript
            .iter()
            .filter_map(|row| row.tool.as_ref())
            .find(|tool| tool.call_id == id)
            .unwrap();
        assert_eq!(tool.status, "interrupted");
        assert!(tool.result.is_none());
        assert!(tool.terminal.is_none());
    }
    assert!(
        model
            .transcript
            .iter()
            .all(|row| !row.text.contains("PRIVATE"))
    );
    assert!(
        model
            .transcript
            .iter()
            .any(|row| row.text.starts_with("AppServer canceled:"))
    );
    assert_eq!(store.load_messages().unwrap(), canonical);
    let reopened = SessionStore::open(store.session_dir().into()).unwrap();
    project(&reopened);
    assert_same_view(&reopened.fresh_read_model().unwrap(), &model);
}
