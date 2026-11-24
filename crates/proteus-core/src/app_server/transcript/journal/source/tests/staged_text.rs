use super::*;
use crate::{
    core::{DisplayRecords, ModelResponseRecorded, TurnOpened},
    domain::{AgentTask, ExchangeId, ThreadId, ToolCall, new_exchange_id},
    model_standard::{CanonicalModelResponse, FinishReason},
};

fn text(value: &str) -> CanonicalMessage {
    CanonicalMessage::text(MessageRole::Assistant, value)
}

fn response(exchange_id: ExchangeId, message: &CanonicalMessage) -> JournalEntry {
    JournalEntry::ModelResponseRecorded(ModelResponseRecorded {
        exchange_id,
        outcome: ModelResponseOutcome::Response {
            response: CanonicalModelResponse::new(message.clone(), vec![], FinishReason::Stop),
        },
    })
}

fn append(message: &CanonicalMessage) -> JournalEntry {
    history(HistoryMutationKind::Append, vec![message.clone()]).entry
}

fn opened() -> JournalEntry {
    JournalEntry::TurnOpened(TurnOpened {
        task: AgentTask::new("inspect", "/tmp".into()),
        intent: None,
        base_history_revision: 0,
        module_epoch: 0,
        config_snapshot: serde_json::json!({}),
    })
}

fn records(
    items: impl IntoIterator<Item = (TurnId, ThreadId, JournalEntry)>,
) -> Vec<JournalRecord> {
    let session_id = new_session_id();
    let mut revision = 0;
    items
        .into_iter()
        .enumerate()
        .map(|(i, (turn, thread, mut entry))| {
            if let JournalEntry::HistoryMutated(mutation) = &mut entry {
                mutation.previous_revision = revision;
                revision += 1;
                mutation.new_revision = revision;
            }
            JournalRecord {
                schema_version: JOURNAL_SCHEMA_VERSION,
                record_id: new_record_id(),
                session_seq: i as u64 + 1,
                timestamp_ms: 100 - i as i64,
                session_id,
                execution_id: None,
                thread_id: Some(thread),
                turn_id: Some(turn),
                entry,
            }
        })
        .collect()
}

#[test]
fn staged_text_witnesses_preserve_early_rows_and_hidden_root_live_commits() {
    let turn = TurnId::from_u128(1);
    let live = TurnId::from_u128(2);
    let root = ThreadId::from_u128(3);
    let superseded = ThreadId::from_u128(4);
    let exchange = new_exchange_id();
    let (a, b, hidden, active) = (text("A"), text("B"), text("superseded"), text("live"));
    let items = records([
        (turn, superseded, opened()),
        (live, root, opened()),
        (turn, superseded, response(exchange, &hidden)),
        (live, root, append(&hidden)),
        (turn, root, response(exchange, &a)),
        (turn, root, append(&text("between response and commit"))),
        (live, root, append(&a)),
        (live, root, response(exchange, &active)),
        (live, root, append(&active)),
        (turn, root, response(exchange, &b)),
        (turn, root, append(&b)),
        (turn, root, opened()), // Final root binding applies to the whole prefix.
    ]);

    let (mut staged, roots) = DisplayRecords::stage_for_test(&items);
    let actual = render_causal(
        &mut |visit| staged.scan(|record| visit(StagedRenderRecord::Journal(record))),
        Some(live),
        false,
        true,
        TranscriptVisibility {
            root_threads: roots.expect("every response has a positive commit witness"),
            certified_text_responses: true,
            ..Default::default()
        },
    )
    .unwrap()
    .into_full();
    let expected = from_records(&items, Some(live)).unwrap();
    assert_eq!(
        serde_json::to_value(&actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(
        actual
            .iter()
            .map(|r| (r.text.as_str(), r.timestamp_ms))
            .collect::<Vec<_>>(),
        [
            ("A", Some(96)),
            ("between response and commit", Some(95)),
            ("B", Some(91))
        ]
    );
}

#[test]
fn missing_or_overlapping_witness_falls_back_even_for_a_duplicate_exchange() {
    let turn = TurnId::from_u128(1);
    let root = ThreadId::from_u128(2);
    let exchange = new_exchange_id();
    let (a, b) = (text("unwitnessed until later"), text("witnessed"));
    for tail in [vec![], vec![append(&a)]] {
        let items = records(
            [
                opened(),
                response(exchange, &a),
                response(exchange, &b),
                append(&b),
            ]
            .into_iter()
            .chain(tail)
            .map(|entry| (turn, root, entry)),
        );
        let (mut staged, roots) = DisplayRecords::stage_for_test(&items);
        assert!(roots.is_none());
        // Ordinary visibility is by exchange: B's commit can also reveal A.
        let rows = transcript_from_scan(|visit| staged.scan(visit), None).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(),
            ["unwitnessed until later", "witnessed"]
        );
    }
    let items = records(
        [
            opened(),
            response(exchange, &a),
            append(&text("different part")),
        ]
        .into_iter()
        .map(|entry| (turn, root, entry)),
    );
    assert!(
        DisplayRecords::stage_for_test(&items).1.is_none(),
        "pending at EOF"
    );
}

#[test]
fn staged_text_rejects_tool_activity_before_inside_or_after_response() {
    let call = ToolCall::new("lookup", "lookup", serde_json::json!({}));
    let tool_message = CanonicalMessage::new(
        MessageRole::Assistant,
        vec![ContentPart::ToolCall { call: call.clone() }],
    );
    let exchange = new_exchange_id();
    let m = text("text");
    let tool_response = JournalEntry::ModelResponseRecorded(ModelResponseRecorded {
        exchange_id: exchange,
        outcome: ModelResponseOutcome::Response {
            response: CanonicalModelResponse::new(
                tool_message.clone(),
                vec![call],
                FinishReason::ToolCalls,
            ),
        },
    });
    for entries in [
        vec![append(&tool_message), response(exchange, &m), append(&m)],
        vec![response(exchange, &m), append(&m), append(&tool_message)],
        vec![tool_response, append(&tool_message)],
    ] {
        let items = records(
            entries
                .into_iter()
                .map(|entry| (TurnId::from_u128(1), ThreadId::from_u128(2), entry)),
        );
        assert!(DisplayRecords::stage_for_test(&items).1.is_none());
    }
}

#[test]
fn staged_mixed_append_still_matches_full_order_and_dates() {
    use crate::model_standard::{CanonicalPart, MessagePhase, PartProvenance, PartScope};

    let mut assistant = CanonicalMessage::new(
        MessageRole::Assistant,
        vec![
            ContentPart::Text {
                text: "first 🦀\n".into(),
            },
            ContentPart::ReasoningSummary {
                text: "a separate thought".into(),
            },
            ContentPart::Text {
                text: "second".into(),
            },
        ],
    )
    .with_phase(MessagePhase::FinalAnswer);
    assistant.parts.push(CanonicalPart::new(
        PartProvenance::Runtime,
        PartScope::Trace,
        ContentPart::Text {
            text: "not part of the display".into(),
        },
    ));
    let item = history(
        HistoryMutationKind::Append,
        vec![
            CanonicalMessage::text(MessageRole::User, "  \t\n"),
            assistant,
            CanonicalMessage::text(MessageRole::User, "another user"),
        ],
    );
    let items = [item];
    let (mut staged, roots) = DisplayRecords::stage_for_test(&items);
    let actual = render_causal(
        &mut |visit| staged.scan(|record| visit(StagedRenderRecord::Journal(record))),
        None,
        true,
        true,
        TranscriptVisibility {
            root_threads: roots.expect("history-only stage"),
            certified_text_responses: true,
            ..Default::default()
        },
    )
    .unwrap()
    .into_full();
    let expected = from_records(&items, None).unwrap();
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
    assert_eq!(
        actual.iter().map(|r| r.role.as_str()).collect::<Vec<_>>(),
        ["reasoning", "assistant", "user"]
    );
    assert!(actual.iter().all(|row| row.timestamp_ms == Some(42)));
}
