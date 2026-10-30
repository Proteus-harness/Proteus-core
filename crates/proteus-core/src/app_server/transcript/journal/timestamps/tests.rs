use super::*;
use crate::{
    core::{HistoryMutated, HistoryMutationKind, JOURNAL_SCHEMA_VERSION, JournalRecord},
    domain::{
        HostedToolActivity, HostedToolStatus, ToolCall, WebSearchAction, new_record_id,
        new_session_id, new_thread_id,
    },
    model_standard::MessageRole,
};

fn projection(
    entries: Vec<(HistoryMutationKind, Vec<CanonicalMessage>, i64)>,
) -> JournalProjection {
    let session = new_session_id();
    let thread = new_thread_id();
    let records = entries
        .into_iter()
        .enumerate()
        .map(
            |(index, (mutation, messages, timestamp_ms))| JournalRecord {
                schema_version: JOURNAL_SCHEMA_VERSION,
                record_id: new_record_id(),
                session_seq: index as u64 + 1,
                timestamp_ms,
                session_id: session,
                execution_id: None,
                thread_id: Some(thread),
                turn_id: None,
                entry: JournalEntry::HistoryMutated(HistoryMutated {
                    previous_revision: index as u64,
                    new_revision: index as u64 + 1,
                    mutation,
                    messages,
                    compaction: None,
                    tool_results: vec![],
                }),
            },
        )
        .collect();
    JournalProjection::build(session, records).unwrap()
}

#[test]
fn scoped_dates_keep_only_requested_message_and_original_part_keys() {
    let erased = (0..64)
        .map(|index| CanonicalMessage::text(MessageRole::User, format!("old {index}")))
        .collect::<Vec<_>>();
    let mut retained = CanonicalMessage::text(MessageRole::Assistant, "new text");
    retained.parts.insert(0, erased[0].parts[0].clone());
    let projection = projection(vec![
        (HistoryMutationKind::Append, erased, 100),
        (HistoryMutationKind::Replace, vec![retained.clone()], 50),
    ]);
    let full = TranscriptTimestamps::from_projection(&projection);
    let scoped = TranscriptTimestamps::for_replay(&projection, 1);
    assert_eq!(full.messages.len(), 65);
    assert_eq!(full.parts.len(), 65);
    assert_eq!(scoped.messages.len(), 1);
    assert_eq!(scoped.parts.len(), 2);
    assert_eq!(scoped.message(&retained), Some(100));
    assert_eq!(scoped.message(&retained), full.message(&retained));
    assert_eq!(
        std::mem::size_of::<RecordedAt>(),
        std::mem::size_of::<(u64, i64)>(),
        "vacant slots must not grow the per-date value",
    );
}

#[test]
fn no_reset_and_small_prefix_large_suffix_do_not_create_a_second_index() {
    let current = (0..64)
        .map(|index| CanonicalMessage::text(MessageRole::Assistant, format!("new {index}")))
        .collect::<Vec<_>>();
    let projection = projection(vec![
        (
            HistoryMutationKind::Append,
            vec![CanonicalMessage::text(MessageRole::User, "old")],
            100,
        ),
        (HistoryMutationKind::Replace, current.clone(), 50),
    ]);
    let full = TranscriptTimestamps::from_projection(&projection);
    let no_reset = TranscriptTimestamps::for_replay(&projection, 0);
    let scoped = TranscriptTimestamps::for_replay(&projection, 1);
    assert_eq!(no_reset.messages.len(), full.messages.len());
    assert_eq!(no_reset.parts.len(), full.parts.len());
    assert_eq!(scoped.messages.len(), 64);
    assert_eq!(scoped.parts.len(), 64);
    for message in &current {
        assert_eq!(scoped.message(message), full.message(message));
        assert_eq!(no_reset.message(message), full.message(message));
    }

    let empty = projection_with_empty_reset();
    let empty_index = TranscriptTimestamps::for_replay(&empty, 1);
    assert!(empty_index.messages.is_empty());
    assert!(empty_index.parts.is_empty());
}

fn projection_with_empty_reset() -> JournalProjection {
    projection(vec![
        (
            HistoryMutationKind::Append,
            vec![CanonicalMessage::text(MessageRole::User, "discarded")],
            100,
        ),
        (HistoryMutationKind::Replace, vec![], 50),
    ])
}

#[test]
fn call_dates_stay_global_without_preseeding_absent_hosted_ids() {
    let old = CanonicalMessage::new(
        MessageRole::Assistant,
        vec![ContentPart::ToolCall {
            call: ToolCall::new("shared-id", "probe", serde_json::json!({})),
        }],
    );
    let current = (0..32)
        .map(|index| {
            let id = if index == 0 {
                "shared-id".into()
            } else {
                format!("hosted-{index}")
            };
            CanonicalMessage::new(
                MessageRole::Assistant,
                vec![ContentPart::HostedToolActivity {
                    activity: HostedToolActivity::WebSearch {
                        id,
                        status: HostedToolStatus::Completed,
                        action: WebSearchAction::Search {
                            queries: vec![],
                            sources: vec![],
                        },
                    },
                }],
            )
        })
        .collect::<Vec<_>>();
    let projection = projection(vec![
        (HistoryMutationKind::Append, vec![old], 100),
        (HistoryMutationKind::Replace, current, 50),
    ]);
    let full = TranscriptTimestamps::from_projection(&projection);
    let scoped = TranscriptTimestamps::for_replay(&projection, 1);
    assert_eq!(scoped.calls.len(), 1);
    assert_eq!(scoped.calls.len(), full.calls.len());
    assert_eq!(scoped.call("shared-id"), Some(100));
    assert_eq!(scoped.call("hosted-1"), None);
    let rows = super::super::journal_transcript_messages(projection.records.clone(), None);
    assert_eq!(rows[0].timestamp_ms, Some(100));
    assert_eq!(rows[1].timestamp_ms, Some(50));
    assert_eq!(
        serde_json::to_value(&rows).unwrap(),
        serde_json::to_value(super::super::replay_transcript(&projection, None, 0)).unwrap(),
    );
}
