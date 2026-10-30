use serde_json::json;

mod borrowed_parts;

use super::*;
use crate::{
    contracts::ModelCallOrigin,
    core::{
        HistoryMutated, JOURNAL_SCHEMA_VERSION, JournalRecord, ModelRequestRecorded,
        ModelResponseRecorded, ToolCallRecorded, ToolResultRecorded, TurnOpened, TurnSettled,
        TurnSettlementStatus,
    },
    domain::{
        AgentTask, ModelRef, ToolCall, ToolResult, new_exchange_id, new_execution_id,
        new_record_id, new_session_id, new_thread_id, new_turn_id,
    },
    model_standard::{CanonicalModelRequest, CanonicalModelResponse, FinishReason, MessageRole},
};

fn record(
    session_id: crate::domain::SessionId,
    execution_id: Option<crate::domain::ExecutionId>,
    thread_id: ThreadId,
    turn_id: TurnId,
    session_seq: u64,
    entry: JournalEntry,
) -> JournalRecord {
    JournalRecord {
        schema_version: JOURNAL_SCHEMA_VERSION,
        record_id: new_record_id(),
        session_seq,
        timestamp_ms: session_seq as i64,
        session_id,
        execution_id,
        thread_id: Some(thread_id),
        turn_id: Some(turn_id),
        entry,
    }
}

#[test]
fn failed_turn_recovers_model_text_tool_card_and_terminal_error_from_journal() {
    let session_id = new_session_id();
    let thread_id = new_thread_id();
    let turn_id = new_turn_id();
    let execution_id = new_execution_id();
    let exchange_id = new_exchange_id();
    let user = CanonicalMessage::text(MessageRole::User, "inspect");
    let call = ToolCall::new("call-1", "shell", json!({"command": "ls"}));
    let response_message = CanonicalMessage::new(
        MessageRole::Assistant,
        vec![
            ContentPart::ReasoningSummary {
                text: "private summary".to_owned(),
            },
            ContentPart::Reasoning {
                text: "private reasoning".to_owned(),
                signature: Some("opaque signature".to_owned()),
            },
            ContentPart::Text {
                text: "checking".to_owned(),
            },
            ContentPart::ToolCall { call: call.clone() },
        ],
    );
    let response = CanonicalModelResponse::new(
        response_message.clone(),
        vec![call.clone()],
        FinishReason::ToolCalls,
    );
    let request = CanonicalModelRequest::new(ModelRef::new("fake", "model"), vec![user.clone()]);
    let result = ToolResult::ok(call.id.clone(), "listing");
    let mut records = vec![
        record(
            session_id,
            Some(execution_id),
            thread_id,
            turn_id,
            1,
            JournalEntry::TurnOpened(TurnOpened {
                intent: None,
                task: AgentTask::new("inspect", "/tmp/workspace".into()),
                base_history_revision: 0,
                module_epoch: 0,
                config_snapshot: json!({}),
            }),
        ),
        record(
            session_id,
            None,
            thread_id,
            turn_id,
            2,
            JournalEntry::HistoryMutated(HistoryMutated {
                previous_revision: 0,
                new_revision: 1,
                tool_results: Vec::new(),
                mutation: HistoryMutationKind::Append,
                messages: vec![user],
                compaction: None,
            }),
        ),
        record(
            session_id,
            Some(execution_id),
            thread_id,
            turn_id,
            3,
            JournalEntry::ModelRequestRecorded(ModelRequestRecorded {
                exchange_id,
                origin: ModelCallOrigin::Direct,
                owner: crate::contracts::OperationOwner::Workflow,
                request,
            }),
        ),
        record(
            session_id,
            Some(execution_id),
            thread_id,
            turn_id,
            4,
            JournalEntry::ModelResponseRecorded(ModelResponseRecorded {
                exchange_id,
                outcome: ModelResponseOutcome::Response { response },
            }),
        ),
        record(
            session_id,
            Some(execution_id),
            thread_id,
            turn_id,
            5,
            JournalEntry::ToolCallRecorded(ToolCallRecorded {
                call: call.clone(),
                phase: ToolCallRecordPhase::Requested {
                    owner: crate::contracts::OperationOwner::Workflow,
                },
            }),
        ),
        record(
            session_id,
            Some(execution_id),
            thread_id,
            turn_id,
            6,
            JournalEntry::ToolCallRecorded(ToolCallRecorded {
                call,
                phase: ToolCallRecordPhase::Resolved {
                    resolution: ToolCallResolution::Allowed,
                },
            }),
        ),
        record(
            session_id,
            Some(execution_id),
            thread_id,
            turn_id,
            7,
            JournalEntry::ToolResultRecorded(ToolResultRecorded { result }),
        ),
        record(
            session_id,
            None,
            thread_id,
            turn_id,
            8,
            JournalEntry::TurnSettled(TurnSettled {
                status: TurnSettlementStatus::Error,
                output: None,
                error: Some("workflow failed after tool execution".to_owned()),
            }),
        ),
    ];
    records.insert(
        3,
        record(
            session_id,
            Some(execution_id),
            thread_id,
            turn_id,
            4,
            JournalEntry::ModelMessageRecorded(
                serde_json::from_value(json!({
                    "exchange_id": exchange_id, "message": response_message,
                }))
                .unwrap(),
            ),
        ),
    );
    for (index, item) in records.iter_mut().enumerate() {
        item.session_seq = index as u64 + 1;
        item.timestamp_ms = item.session_seq as i64;
    }
    let live_projection =
        JournalProjection::build(session_id, records[..7].to_vec()).expect("live projection");
    let live_transcript =
        journal_transcript_messages(live_projection.records.clone(), Some(turn_id));
    assert!(
        live_transcript.is_empty(),
        "live turn belongs entirely to the event projection until settlement"
    );

    let projection = JournalProjection::build(session_id, records).expect("projection");

    let transcript = journal_transcript_messages(projection.records.clone(), None);

    assert!(
        transcript
            .iter()
            .any(|message| message.role == "user" && message.text == "inspect")
    );
    assert!(
        transcript
            .iter()
            .any(|message| message.role == "assistant" && message.text == "checking")
    );
    assert!(
        transcript
            .iter()
            .all(|message| !message.text.contains("private"))
    );
    assert_eq!(
        transcript
            .iter()
            .find(|row| row.text == "checking")
            .unwrap()
            .timestamp_ms,
        Some(4)
    );
    let tool = transcript
        .iter()
        .find_map(|message| message.tool.as_ref())
        .expect("tool card");
    assert_eq!(tool.call_id, "call-1");
    assert_eq!(tool.status, "done");
    assert_eq!(tool.result.as_deref(), Some("listing"));
    let terminal = transcript.last().expect("terminal failure message");
    assert_eq!(terminal.role, "system");
    assert_eq!(
        terminal.text,
        "AppServer error: workflow failed after tool execution"
    );
    assert!(terminal.tool.is_none());
}

#[test]
fn canceled_turn_without_error_recovers_terminal_system_message() {
    let session_id = new_session_id();
    let thread_id = new_thread_id();
    let turn_id = new_turn_id();
    let execution_id = new_execution_id();
    let records = vec![
        record(
            session_id,
            Some(execution_id),
            thread_id,
            turn_id,
            1,
            JournalEntry::TurnOpened(TurnOpened {
                intent: None,
                task: AgentTask::new("cancel", "/tmp/workspace".into()),
                base_history_revision: 0,
                module_epoch: 0,
                config_snapshot: json!({}),
            }),
        ),
        record(
            session_id,
            None,
            thread_id,
            turn_id,
            2,
            JournalEntry::TurnSettled(TurnSettled {
                status: TurnSettlementStatus::Canceled,
                output: None,
                error: None,
            }),
        ),
    ];

    let projection = JournalProjection::build(session_id, records).expect("projection");
    let transcript = journal_transcript_messages(projection.records.clone(), None);

    assert_eq!(transcript.len(), 1);
    assert_eq!(transcript[0].role, "system");
    assert_eq!(
        transcript[0].text,
        "AppServer canceled: turn canceled by client"
    );
    assert!(transcript[0].tool.is_none());
}

#[test]
fn replacement_and_compaction_keep_first_dates_of_retained_message_or_part_ids() {
    for retain_message_id in [false, true] {
        let session_id = new_session_id();
        let thread_id = new_thread_id();
        let turn_id = new_turn_id();
        let original = CanonicalMessage::text(MessageRole::User, "accepted");
        let mut retained = original.clone();
        if retain_message_id {
            // Replacing a message's parts must not make the existing message new.
            retained.parts = CanonicalMessage::text(MessageRole::User, "accepted").parts;
        } else {
            retained.id = crate::domain::new_message_id();
        }
        let mut compaction = crate::domain::HistoryCompactionReport::unchanged(1, None);
        compaction.changed = true;
        let mut records = Vec::new();
        for (index, (message, compacted)) in [
            (original, false),
            (retained.clone(), false),
            (retained, true),
        ]
        .into_iter()
        .enumerate()
        {
            let sequence = index as u64 + 1;
            let mut item = record(
                session_id,
                None,
                thread_id,
                turn_id,
                sequence,
                JournalEntry::HistoryMutated(HistoryMutated {
                    previous_revision: sequence - 1,
                    new_revision: sequence,
                    mutation: if index == 0 {
                        HistoryMutationKind::Append
                    } else {
                        HistoryMutationKind::Replace
                    },
                    messages: vec![message],
                    compaction: compacted.then(|| compaction.clone()),
                    tool_results: vec![],
                }),
            );
            item.turn_id = None;
            // A backwards clock adjustment is not an earlier creation event.
            item.timestamp_ms = 100 - index as i64;
            records.push(item);
            let projection = JournalProjection::build(session_id, records.clone()).unwrap();
            let rows = journal_transcript_messages(projection.records.clone(), None);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].timestamp_ms, Some(100));
        }
    }
}

mod replay_frontier;
