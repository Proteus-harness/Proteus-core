use super::*;
use crate::{
    contracts::{ModelCallOrigin, OperationOwner},
    core::{
        HistoryMutated, JOURNAL_SCHEMA_VERSION, ModelMessageRecorded, ModelRequestRecorded,
        ModelResponseRecorded, ToolCallRecordPhase, ToolCallRecorded, ToolResultRecorded,
        TurnOpened, TurnSettled, TurnSettlementStatus,
    },
    domain::{
        AgentTask, Citation, HostedToolActivity, HostedToolStatus, ImageRef, ToolCall, ToolResult,
        WebSearchAction, new_exchange_id, new_record_id, new_session_id, new_thread_id,
        new_turn_id,
    },
    model_standard::{
        CanonicalModelRequest, CanonicalModelResponse, CanonicalPart, FinishReason, MessageRole,
        PartProvenance, PartScope,
    },
};
use serde_json::json;

mod checkpoints;

fn append_records(
    records: &[JournalRecord],
    split: usize,
) -> Result<Option<SessionTranscriptAppend>> {
    let last = split
        .checked_sub(1)
        .map_or(0, |index| records[index].session_seq);
    let previous = validated_history(&records[..split]);
    let final_history = validated_history(records);
    // Some focused renderer-only fixtures deliberately omit unrelated strict
    // lifecycle facts. Never use them to certify a retained checkpoint prefix.
    if (previous.is_none() || final_history.is_none())
        && records[split..].iter().any(|record| {
            matches!(&record.entry, JournalEntry::HistoryMutated(mutation)
                if mutation.mutation == HistoryMutationKind::Checkpoint)
        })
    {
        return Ok(None);
    }
    let baseline = previous
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(history_message_sha256)
        .collect::<Result<Vec<_>>>()?;
    let final_len = match (&previous, &final_history) {
        (Some(_), Some(history)) => history.len(),
        _ => baseline.len() + append_count(&records[split..]),
    };
    let Some(conversation_appended) = independent_append(
        |visit| {
            for record in records {
                if is_display_kind(record.entry.kind()) {
                    visit(record.clone())?;
                }
            }
            Ok(())
        },
        last,
        &baseline,
        final_len,
    )?
    else {
        return Ok(None);
    };
    let transcript = transcript_from_scan(
        |visit| {
            for record in &records[split..] {
                if is_display_kind(record.entry.kind()) {
                    let mut record = record.clone();
                    strip_checkpoint_baseline(&mut record, baseline.len())?;
                    visit(record)?;
                }
            }
            Ok(())
        },
        None,
    )?;
    Ok(Some(SessionTranscriptAppend {
        transcript,
        conversation_appended,
    }))
}

fn validated_history(records: &[JournalRecord]) -> Option<Vec<CanonicalMessage>> {
    if records.is_empty() {
        return Some(vec![]);
    }
    crate::core::JournalProjection::build(records[0].session_id, records.to_vec())
        .ok()
        .map(|projection| projection.history)
}

fn append_count(records: &[JournalRecord]) -> usize {
    records
        .iter()
        .filter_map(|record| match &record.entry {
            JournalEntry::HistoryMutated(item) => Some(item.messages.len()),
            _ => None,
        })
        .sum()
}

/// Also exercised by the existing validated journal fixture suite: late commit,
/// final result vs effect, root/child ownership, reset, compaction, citations,
/// images, hidden parts and clock adjustments all pass through this oracle.
pub(in crate::app_server::transcript::journal) fn assert_split_equivalence(
    records: &[JournalRecord],
) {
    let expected = super::super::source::from_records(records, None).unwrap();
    for split in 0..=records.len() {
        let Some(append) = append_records(records, split).unwrap() else {
            continue; // The caller must use the unchanged complete renderer.
        };
        let mut actual = super::super::source::from_records(&records[..split], None).unwrap();
        actual.extend(append.transcript);
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(&expected).unwrap(),
            "independent append diverged at split {split}/{}",
            records.len(),
        );
        let count = match (
            validated_history(&records[..split]),
            validated_history(records),
        ) {
            (Some(previous), Some(current)) => current.len() - previous.len(),
            _ => append_count(&records[split..]),
        };
        assert_eq!(append.conversation_appended, count);
    }
}

fn record(sequence: u64, entry: JournalEntry) -> JournalRecord {
    JournalRecord {
        schema_version: JOURNAL_SCHEMA_VERSION,
        record_id: new_record_id(),
        session_id: new_session_id(),
        session_seq: sequence,
        timestamp_ms: sequence as i64 * 100,
        execution_id: None,
        thread_id: None,
        turn_id: None,
        entry,
    }
}

fn history(sequence: u64, messages: Vec<CanonicalMessage>) -> JournalRecord {
    record(
        sequence,
        JournalEntry::HistoryMutated(HistoryMutated {
            previous_revision: sequence - 1,
            new_revision: sequence,
            mutation: HistoryMutationKind::Append,
            messages,
            compaction: None,
            tool_results: vec![],
        }),
    )
}

fn text(role: MessageRole, value: &str) -> CanonicalMessage {
    CanonicalMessage::text(role, value)
}

#[test]
fn independent_user_messages_and_an_empty_suffix_hit_the_append_path() {
    let records = vec![
        history(1, vec![text(MessageRole::User, "first")]),
        history(2, vec![text(MessageRole::User, "second")]),
    ];
    assert_split_equivalence(&records);
    let appended = append_records(&records, 1).unwrap().unwrap();
    assert_eq!(appended.conversation_appended, 1);
    assert_eq!(appended.transcript[0].text, "second");
    let empty = append_records(&records, 2).unwrap().unwrap();
    assert!(empty.transcript.is_empty());
    assert_eq!(empty.conversation_appended, 0);
}

#[test]
fn a_whole_new_settled_turn_with_intra_suffix_repeats_is_independent() {
    let turn = new_turn_id();
    let thread = new_thread_id();
    let exchange = new_exchange_id();
    let call = ToolCall::new("new-call", "shell", json!({"cmd": "pwd"}));
    let user = text(MessageRole::User, "new turn");
    let response = CanonicalMessage::new(
        MessageRole::Assistant,
        vec![
            ContentPart::Text {
                text: "checking".into(),
            },
            ContentPart::ToolCall { call: call.clone() },
        ],
    );
    let entries = vec![
        JournalEntry::TurnOpened(TurnOpened {
            task: AgentTask::new("new turn", "/synthetic".into()),
            intent: None,
            base_history_revision: 1,
            module_epoch: 0,
            config_snapshot: json!({}),
        }),
        JournalEntry::HistoryMutated(HistoryMutated {
            previous_revision: 1,
            new_revision: 2,
            mutation: HistoryMutationKind::Append,
            messages: vec![user.clone()],
            compaction: None,
            tool_results: vec![],
        }),
        JournalEntry::ModelRequestRecorded(ModelRequestRecorded {
            exchange_id: exchange,
            origin: ModelCallOrigin::Direct,
            owner: OperationOwner::Workflow,
            request: CanonicalModelRequest::new(
                crate::domain::ModelRef::new("fake", "fake"),
                vec![user],
            ),
        }),
        JournalEntry::ModelMessageRecorded(ModelMessageRecorded {
            exchange_id: exchange,
            message: response.clone(),
        }),
        JournalEntry::ModelResponseRecorded(ModelResponseRecorded {
            exchange_id: exchange,
            outcome: ModelResponseOutcome::Response {
                response: CanonicalModelResponse::new(
                    response,
                    vec![call.clone()],
                    FinishReason::ToolCalls,
                ),
            },
        }),
        JournalEntry::ToolCallRecorded(ToolCallRecorded {
            call: call.clone(),
            phase: ToolCallRecordPhase::Requested {
                owner: OperationOwner::Workflow,
            },
        }),
        JournalEntry::ToolEffectRecorded(ToolResultRecorded {
            result: ToolResult::ok(call.id.clone(), "effect"),
        }),
        JournalEntry::ToolResultRecorded(ToolResultRecorded {
            result: ToolResult::ok(call.id, "final result"),
        }),
        JournalEntry::TurnSettled(TurnSettled {
            status: TurnSettlementStatus::Success,
            output: None,
            error: None,
        }),
    ];
    let mut records = vec![history(1, vec![text(MessageRole::User, "cached")])];
    for entry in entries {
        let mut next = record(records.len() as u64 + 1, entry);
        next.turn_id = Some(turn);
        next.thread_id = Some(thread);
        records.push(next);
    }
    assert_split_equivalence(&records);
    let appended = append_records(&records, 1).unwrap().unwrap();
    assert_eq!(appended.conversation_appended, 1);
    assert!(appended.transcript.iter().any(|row| row.text == "checking"));
    let tool = appended
        .transcript
        .iter()
        .find_map(|row| row.tool.as_ref())
        .unwrap();
    assert_eq!(tool.result.as_deref(), Some("final result"));
    assert_eq!(tool.status, "done");
    assert!(
        append_records(&records, 2).unwrap().is_none(),
        "same open turn overlaps"
    );
}

#[test]
fn old_message_part_and_hidden_rehosted_identities_require_full_projection() {
    let original = text(MessageRole::Assistant, "old");
    let mut same_message = text(MessageRole::Assistant, "new parts");
    same_message.id = original.id;
    let mut same_part = original.clone();
    same_part.id = crate::domain::new_message_id();
    for repeated in [same_message, same_part] {
        let records = vec![
            history(1, vec![original.clone()]),
            history(2, vec![repeated]),
        ];
        assert!(append_records(&records, 1).unwrap().is_none());
        assert_split_equivalence(&records);
    }
    let hidden = CanonicalMessage::from_parts(
        MessageRole::Assistant,
        vec![CanonicalPart::new(
            PartProvenance::ContextBuilder,
            PartScope::Request,
            ContentPart::Text {
                text: "hidden".into(),
            },
        )],
    );
    let mut rehosted = hidden.clone();
    rehosted.id = crate::domain::new_message_id();
    rehosted.parts[0].scope = PartScope::Conversation;
    let records = vec![history(1, vec![hidden]), history(2, vec![rehosted])];
    assert!(append_records(&records, 1).unwrap().is_none());
    assert_split_equivalence(&records);
}

#[test]
fn an_uncommitted_model_message_cannot_gain_a_new_date_on_late_commit() {
    let message = text(MessageRole::Assistant, "late commit");
    let records = vec![
        record(
            1,
            JournalEntry::ModelMessageRecorded(ModelMessageRecorded {
                exchange_id: new_exchange_id(),
                message: message.clone(),
            }),
        ),
        history(2, vec![message]),
    ];
    assert!(append_records(&records, 1).unwrap().is_none());
    assert_split_equivalence(&records);
}

#[test]
fn late_final_results_and_hosted_card_updates_require_full_projection() {
    let call = ToolCall::new("old-call", "shell", json!({}));
    for first in [
        record(
            1,
            JournalEntry::ToolEffectRecorded(ToolResultRecorded {
                result: ToolResult::ok(call.id.clone(), "effect"),
            }),
        ),
        history(
            1,
            vec![CanonicalMessage::new(
                MessageRole::Assistant,
                vec![ContentPart::HostedToolActivity {
                    activity: HostedToolActivity::WebSearch {
                        id: call.id.clone(),
                        status: HostedToolStatus::Completed,
                        action: WebSearchAction::Search {
                            queries: vec![],
                            sources: vec![],
                        },
                    },
                }],
            )],
        ),
    ] {
        let records = vec![
            first,
            record(
                2,
                JournalEntry::ToolResultRecorded(ToolResultRecorded {
                    result: ToolResult::ok(call.id.clone(), "final"),
                }),
            ),
        ];
        assert!(append_records(&records, 1).unwrap().is_none());
        assert_split_equivalence(&records);
    }
}

#[test]
fn citations_always_fall_back_and_images_in_new_messages_can_append() {
    let old = history(1, vec![text(MessageRole::User, "old")]);
    let citation = CanonicalMessage::new(
        MessageRole::Assistant,
        vec![ContentPart::Citation {
            citation: Citation::Url {
                start_index: 0,
                end_index: 1,
                title: "source".into(),
                url: "https://example.com".into(),
            },
        }],
    );
    let records = vec![old.clone(), history(2, vec![citation])];
    assert!(append_records(&records, 1).unwrap().is_none());
    assert_split_equivalence(&records);

    let image = CanonicalMessage::new(
        MessageRole::User,
        vec![ContentPart::Image {
            image: ImageRef {
                id: "image".into(),
                name: "image.png".into(),
                mime_type: "image/png".into(),
                path: "/synthetic/image".into(),
            },
        }],
    );
    let records = vec![old, history(2, vec![image])];
    assert_eq!(
        append_records(&records, 1).unwrap().unwrap().transcript[0]
            .images
            .len(),
        1
    );
    assert_split_equivalence(&records);
}

#[test]
fn replaces_unverified_checkpoints_and_compactions_require_the_complete_renderer() {
    for kind in [
        HistoryMutationKind::Replace,
        HistoryMutationKind::Checkpoint,
    ] {
        let mut changed = history(2, vec![text(MessageRole::User, "changed")]);
        if let JournalEntry::HistoryMutated(mutation) = &mut changed.entry {
            mutation.mutation = kind;
        }
        let records = vec![history(1, vec![text(MessageRole::User, "cached")]), changed];
        assert!(append_records(&records, 1).unwrap().is_none());
        assert_split_equivalence(&records);
    }
    let mut changed = history(1, vec![]);
    if let JournalEntry::HistoryMutated(mutation) = &mut changed.entry {
        mutation.compaction = Some(crate::domain::HistoryCompactionReport::unchanged(0, None));
    }
    assert!(append_records(&[changed], 0).unwrap().is_none());
}

#[test]
fn a_scan_failure_is_reported_even_after_the_suffix_is_known_to_overlap() {
    let first = history(1, vec![text(MessageRole::User, "cached")]);
    let mut repeated = first.clone();
    repeated.session_seq = 2;
    let error = independent_append(
        |visit| {
            visit(first.clone())?;
            visit(repeated.clone())?;
            anyhow::bail!("changed source");
        },
        1,
        &[],
        1,
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "changed source");
}
