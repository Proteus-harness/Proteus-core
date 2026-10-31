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
        JournalEntry, JournalKind, JournalReadSnapshot, JournalRecord, ModelResponseOutcome,
        ToolCallRecordPhase,
    },
    domain::TurnId,
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
    transcript_from_scan(
        |visit| source.scan_selected(is_display_kind, visit),
        live_turn_id,
    )
}

// Full strict validation already hydrated every entry. Repeated display passes
// may omit unrelated request/hook payloads; the fixed prefix and durable
// session_seq keep frontier comparisons independent of those omissions.
fn is_display_kind(kind: JournalKind) -> bool {
    matches!(
        kind,
        JournalKind::HistoryMutated
            | JournalKind::ModelMessageRecorded
            | JournalKind::ModelResponseRecorded
            | JournalKind::ToolCallRecorded
            | JournalKind::ToolResultRecorded
            | JournalKind::ToolEffectRecorded
            | JournalKind::TurnOpened
            | JournalKind::TurnSettled
    )
}

fn transcript_from_scan(
    mut scan: impl FnMut(&mut dyn FnMut(JournalRecord) -> Result<()>) -> Result<()>,
    live_turn_id: Option<TurnId>,
) -> Result<Vec<AppTranscriptMessage>> {
    let mut start = 0;
    let mut visibility = TranscriptVisibility::default();
    let mut committed_parts = std::collections::HashSet::new();
    let mut final_results = std::collections::HashSet::new();
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
        match &record.entry {
            JournalEntry::HistoryMutated(mutation) => {
                for message in &mutation.messages {
                    committed_parts.extend(message.parts.iter().map(|part| part.part_id));
                }
            }
            JournalEntry::ToolResultRecorded(tool) => {
                final_results.insert(tool.result.call_id.clone());
            }
            _ => {}
        }
        Ok(())
    })?;

    // All lifecycle calls, including those before the frontier, can make a
    // replayed response visible. Timestamp keys only need replayed messages.
    let mut timestamps = TranscriptTimestamps::default();
    scan(&mut |record| {
        if let JournalEntry::ToolCallRecorded(tool) = &record.entry
            && matches!(
                tool.phase,
                ToolCallRecordPhase::Requested {
                    owner: OperationOwner::Workflow
                }
            )
            && visibility.is_root_record(record.thread_id, record.turn_id)
        {
            visibility.visible_tool_calls.insert(tool.call.id.clone());
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
                .any(|call| visibility.visible_tool_calls.contains(&call.id));
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
                if is_display_kind(record.entry.kind()) {
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

    #[test]
    fn a_late_scan_error_does_not_return_partially_rendered_rows() {
        use crate::{
            core::{HistoryMutated, JOURNAL_SCHEMA_VERSION},
            domain::{new_record_id, new_session_id},
            model_standard::{CanonicalMessage, MessageRole},
        };
        let record = JournalRecord {
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
                mutation: HistoryMutationKind::Append,
                messages: vec![CanonicalMessage::text(MessageRole::User, "visible")],
                compaction: None,
                tool_results: vec![],
            }),
        };
        let mut passes = 0;
        let error = transcript_from_scan(
            |visit| {
                passes += 1;
                visit(record.clone())?;
                anyhow::ensure!(passes != 4, "failure after rendering a row");
                Ok(())
            },
            None,
        )
        .unwrap_err();
        assert_eq!(passes, 4);
        assert_eq!(error.to_string(), "failure after rendering a row");
    }

    #[test]
    fn scan_errors_in_every_pass_prevent_publication() {
        for failing_pass in 1..=4 {
            let mut passes = 0;
            let error = transcript_from_scan(
                |_| {
                    passes += 1;
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
