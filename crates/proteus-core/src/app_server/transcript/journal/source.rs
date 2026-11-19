//! Payload-bounded transcript projection over a fixed, already validated prefix.
//!
//! Forward references (late commits, root thread attribution, final tool results)
//! require global facts. Keep only their identities between passes; each hydrated
//! journal payload is dropped before reading the next one. The returned rows are
//! unpublished until every pass succeeds.
use anyhow::Result;

use crate::{
    contracts::OperationOwner,
    core::{
        JournalEntry, JournalReadSnapshot, JournalRecord, ModelResponseOutcome, ToolCallRecordPhase,
    },
    domain::TurnId,
    model_standard::ContentPart,
};

use super::{
    AppTranscriptMessage, HistoryMutationKind, TranscriptProjectionState, TranscriptReplay,
    TranscriptTimestamps, TranscriptVisibility,
};

/// The caller must strictly validate the entire snapshot before invoking this
/// renderer. A reset is only a render frontier, never a validation frontier.
pub(crate) fn journal_transcript_from_source(
    source: &mut JournalReadSnapshot,
    live_turn_id: Option<TurnId>,
) -> Result<Vec<AppTranscriptMessage>> {
    transcript_from_scan(|visit| source.scan_display(visit), live_turn_id)
}

pub(super) fn transcript_from_scan(
    mut scan: impl FnMut(&mut dyn FnMut(JournalRecord) -> Result<()>) -> Result<()>,
    live_turn_id: Option<TurnId>,
) -> Result<Vec<AppTranscriptMessage>> {
    let mut start = 0;
    let mut history_only = true;
    let mut visibility = TranscriptVisibility::default();
    let mut committed_parts = std::collections::HashSet::new();
    let mut final_results = std::collections::HashSet::new();
    let mut subagent_bindings = std::collections::HashMap::new();
    scan(&mut |record| {
        if !(live_turn_id.is_some() && record.turn_id == live_turn_id)
            && matches!(
                &record.entry,
                JournalEntry::HistoryMutated(mutation)
                    if mutation.mutation == HistoryMutationKind::Replace
                        && mutation.compaction.is_none()
            )
        {
            start = record.session_seq;
        }
        if matches!(record.entry, JournalEntry::TurnOpened(_))
            && let Some(turn) = record.turn_id
            && let Some(thread) = record.thread_id
        {
            visibility.root_threads.insert(turn, thread);
        }
        history_only &= matches!(record.entry, JournalEntry::HistoryMutated(_));
        match &record.entry {
            JournalEntry::HistoryMutated(mutation) => {
                for message in &mutation.messages {
                    for part in &message.parts {
                        committed_parts.insert(part.part_id);
                        // A hosted card can take its date from a later tool call.
                        if matches!(part.payload, ContentPart::HostedToolActivity { .. }) {
                            history_only = false;
                        }
                    }
                }
            }
            JournalEntry::SubagentSessionBoundRecorded(binding) => {
                subagent_bindings.insert(binding.source_call_id.clone(), binding.clone());
            }
            JournalEntry::ToolResultRecorded(tool) => {
                final_results.insert(tool.result.call_id.clone());
            }
            _ => {}
        }
        Ok(())
    })?;

    // History-only streams have no forward visibility dependencies. With no
    // replay frontier every message and part is in scope, so collecting dates
    // immediately before rendering each record yields the same first occurrence
    // as collecting the full date index first. In particular the minimum is by
    // sequence, not wall clock. Resets retain the scoped, bounded index below.
    if history_only && start == 0 {
        drop(committed_parts);
        let mut replay = TranscriptReplay {
            state: TranscriptProjectionState::default(),
            visibility,
            final_results,
            subagent_bindings,
        };
        scan(&mut |record| {
            replay.state.timestamps.collect_record::<false>(&record);
            replay.render_record(&record, live_turn_id);
            Ok(())
        })?;
        return Ok(replay.finish());
    }

    // Workflow lifecycle calls, including those before the frontier, can make
    // a replayed model response visible. Nested calls only reveal their own
    // display rows. Timestamp keys only need replayed messages.
    let mut timestamps = TranscriptTimestamps::default();
    scan(&mut |record| {
        if let JournalEntry::ToolCallRecorded(tool) = &record.entry
            && let ToolCallRecordPhase::Requested { owner } = &tool.phase
            && visibility.is_root_record(record.thread_id, record.turn_id)
        {
            visibility
                .display_tool_calls
                .entry(record.turn_id.expect("root record has a turn"))
                .or_default()
                .insert(tool.call.id.clone());
            if matches!(owner, OperationOwner::Workflow) {
                visibility.workflow_root_calls.insert(tool.call.id.clone());
            }
        }
        if record.session_seq >= start {
            match &record.entry {
                JournalEntry::HistoryMutated(mutation) => {
                    for message in &mutation.messages {
                        timestamps.require_message(message);
                    }
                }
                JournalEntry::ModelResponseRecorded(response) => {
                    if let ModelResponseOutcome::Response { response } = &response.outcome {
                        for message in &response.messages {
                            timestamps.require_message(message);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    })?;

    // Resolve first occurrences in journal order, including model-message
    // records and retained parts rehosted by replacement or compaction.
    scan(&mut |record| {
        timestamps.collect_record::<true>(&record);
        if record.session_seq >= start
            && visibility.is_root_record(record.thread_id, record.turn_id)
            && let JournalEntry::ModelResponseRecorded(response) = &record.entry
            && let ModelResponseOutcome::Response { response: model } = &response.outcome
        {
            let committed = model
                .messages
                .iter()
                .flat_map(|message| &message.parts)
                .any(|part| committed_parts.contains(&part.part_id));
            let executed = model
                .tool_calls
                .iter()
                .any(|call| visibility.workflow_root_calls.contains(&call.id));
            if committed || executed {
                visibility
                    .visible_model_responses
                    .insert(response.exchange_id);
            }
        }
        Ok(())
    })?;
    drop(committed_parts);

    let mut replay = TranscriptReplay {
        state: TranscriptProjectionState {
            timestamps,
            ..Default::default()
        },
        visibility,
        final_results,
        subagent_bindings,
    };
    scan(&mut |record| {
        if record.session_seq >= start {
            replay.render_record(&record, live_turn_id);
        }
        Ok(())
    })?;
    Ok(replay.finish())
}

#[cfg(test)]
pub(super) fn from_records(
    records: &[JournalRecord],
    live_turn_id: Option<TurnId>,
) -> Result<Vec<AppTranscriptMessage>> {
    transcript_from_scan(
        |visit| {
            for record in records {
                if crate::core::is_display_kind(record.entry.kind()) {
                    visit(record.clone())?;
                }
            }
            Ok(())
        },
        live_turn_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::{HistoryMutated, JOURNAL_SCHEMA_VERSION},
        domain::{new_record_id, new_session_id},
        model_standard::{CanonicalMessage, MessageRole},
    };

    fn history(kind: HistoryMutationKind, messages: Vec<CanonicalMessage>) -> JournalRecord {
        JournalRecord {
            schema_version: JOURNAL_SCHEMA_VERSION,
            record_id: new_record_id(),
            session_seq: 1,
            timestamp_ms: 42,
            session_id: new_session_id(),
            execution_id: None,
            thread_id: None,
            turn_id: None,
            entry: JournalEntry::HistoryMutated(HistoryMutated {
                previous_revision: 0,
                new_revision: 1,
                mutation: kind,
                messages,
                compaction: None,
                tool_results: vec![],
            }),
        }
    }

    #[test]
    fn failures_in_every_scan_discard_even_rendered_rows() {
        for (kind, expected_passes) in [
            (HistoryMutationKind::Append, 2),
            (HistoryMutationKind::Replace, 4),
        ] {
            let record = history(
                kind,
                vec![CanonicalMessage::text(MessageRole::User, "visible")],
            );
            for failing_pass in 1..=expected_passes {
                let mut passes = 0;
                let error = transcript_from_scan(
                    |visit| {
                        passes += 1;
                        visit(record.clone())?;
                        anyhow::ensure!(passes != failing_pass, "injected scan failure");
                        Ok(())
                    },
                    None,
                )
                .unwrap_err();
                assert_eq!(passes, failing_pass);
                assert_eq!(error.to_string(), "injected scan failure");
            }
        }
    }

    fn counted_replay(records: &[JournalRecord]) -> (Vec<AppTranscriptMessage>, usize) {
        let mut passes = 0;
        let rows = transcript_from_scan(
            |visit| {
                passes += 1;
                for record in records {
                    visit(record.clone())?;
                }
                Ok(())
            },
            None,
        )
        .unwrap();
        let reference =
            super::super::TranscriptReplay::new(records, 0).render(records.iter(), None);
        assert_eq!(
            serde_json::to_value(&rows).unwrap(),
            serde_json::to_value(reference).unwrap()
        );
        (rows, passes)
    }

    #[test]
    fn causal_history_dates_survive_rehosting_and_a_clock_step() {
        let original = CanonicalMessage::text(MessageRole::Assistant, "part one");
        let mut latest = CanonicalMessage::text(MessageRole::Assistant, "part two");
        latest.parts.push(original.parts[0].clone());
        let first = history(HistoryMutationKind::Append, vec![original]);
        let mut second = history(HistoryMutationKind::Replace, vec![latest]);
        let JournalEntry::HistoryMutated(mutation) = &mut second.entry else {
            unreachable!()
        };
        let mut report = crate::domain::HistoryCompactionReport::unchanged(0, None);
        report.changed = true;
        mutation.compaction = Some(report);
        mutation.previous_revision = 1;
        mutation.new_revision = 2;
        second.session_id = first.session_id;
        second.session_seq = 2;
        second.timestamp_ms = 15;
        let projection =
            crate::core::JournalProjection::build(first.session_id, vec![first, second]).unwrap();
        let (rows, passes) = counted_replay(&projection.records);
        assert_eq!(passes, 2);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].text, "part two");
        assert_eq!(rows[1].timestamp_ms, Some(42));
    }

    #[test]
    fn a_hosted_card_can_take_its_date_from_a_future_call() {
        use crate::domain::{HostedToolActivity, HostedToolStatus, ToolCall, WebSearchAction};
        let hosted = CanonicalMessage::new(
            MessageRole::Assistant,
            vec![ContentPart::HostedToolActivity {
                activity: HostedToolActivity::WebSearch {
                    id: "shared".into(),
                    status: HostedToolStatus::Completed,
                    action: WebSearchAction::Search {
                        queries: vec![],
                        sources: vec![],
                    },
                },
            }],
        );
        let call = CanonicalMessage::new(
            MessageRole::Assistant,
            vec![ContentPart::ToolCall {
                call: ToolCall::new("shared", "tool", serde_json::json!({})),
            }],
        );
        let first = history(HistoryMutationKind::Append, vec![hosted]);
        let mut second = history(HistoryMutationKind::Append, vec![call]);
        let JournalEntry::HistoryMutated(mutation) = &mut second.entry else {
            unreachable!()
        };
        mutation.previous_revision = 1;
        mutation.new_revision = 2;
        second.session_id = first.session_id;
        second.session_seq = 2;
        second.timestamp_ms = 15;
        let projection =
            crate::core::JournalProjection::build(first.session_id, vec![first, second]).unwrap();
        let (rows, passes) = counted_replay(&projection.records);
        assert_eq!(passes, 4);
        assert_eq!(rows[0].timestamp_ms, Some(15));
    }
}
