//! Conservative append projection for a previously published transcript.
//!
//! The full strict journal validator runs first. These temporary identity sets
//! establish that the new records cannot alter any old row or its visibility,
//! dates, deduplication, result, citation or image attachment. Ambiguous suffixes
//! use the existing complete projection instead of guessing a repair boundary.
use std::collections::HashSet;

use anyhow::Result;

use crate::{
    core::{
        HistoryMutationKind, JournalEntry, JournalReadCursor, JournalReadSnapshot, JournalRecord,
        ModelResponseOutcome, SessionTranscriptAppend, history_message_sha256,
    },
    domain::{CallId, ExchangeId, MessageId, PartId, TurnId},
    model_standard::{CanonicalMessage, ContentPart},
};

use super::source::{is_display_kind, transcript_from_scan};

/// Only call after strict validation of this snapshot and verification that
/// `previous` identifies its exact previously projected prefix. `baseline`
/// fingerprints come from the strict validator's active history at that cursor,
/// including captured tool results; `final_history_len` is its final history.
/// A checkpoint may retain that exact prefix and add independent new facts.
/// `None` requires the complete renderer; no partial rows are published.
pub(crate) fn journal_transcript_append_from_source(
    source: &mut JournalReadSnapshot,
    previous: &JournalReadCursor,
    baseline: &[[u8; 32]],
    final_history_len: usize,
) -> Result<Option<SessionTranscriptAppend>> {
    let Some(conversation_appended) = independent_append(
        |visit| source.scan_selected(is_display_kind, visit),
        previous.last_session_seq(),
        baseline,
        final_history_len,
    )?
    else {
        return Ok(None);
    };
    let transcript = transcript_from_scan(
        |visit| {
            source.scan_selected_after(previous, is_display_kind, |mut record| {
                strip_checkpoint_baseline(&mut record, baseline.len())?;
                visit(record)
            })
        },
        None,
    )?;
    Ok(Some(SessionTranscriptAppend {
        transcript,
        conversation_appended,
    }))
}

fn independent_append(
    mut scan: impl FnMut(&mut dyn FnMut(JournalRecord) -> Result<()>) -> Result<()>,
    last_session_seq: u64,
    baseline: &[[u8; 32]],
    final_history_len: usize,
) -> Result<Option<usize>> {
    let mut prefix = PrefixIdentities::default();
    let conversation_appended = final_history_len.checked_sub(baseline.len());
    let mut independent = conversation_appended.is_some();
    scan(&mut |mut record| {
        if record.session_seq <= last_session_seq {
            prefix.visit(&record, false);
        } else {
            // Continue the scan after an overlap: raw-prefix mutation and read
            // errors must still be reported before falling back or publishing.
            if let JournalEntry::HistoryMutated(mutation) = &record.entry {
                if mutation.compaction.is_some() {
                    independent = false;
                }
                match mutation.mutation {
                    HistoryMutationKind::Append => {}
                    HistoryMutationKind::Replace => independent = false,
                    HistoryMutationKind::Checkpoint => {
                        if mutation.messages.len() < baseline.len() {
                            independent = false;
                        } else {
                            for (message, expected) in mutation.messages.iter().zip(baseline) {
                                independent &= &history_message_sha256(message)? == expected;
                                // Captured results may enter canonical history
                                // before their first explicit history commit.
                                // Committing one now could reveal an old model
                                // response even when its canonical SHA agrees.
                                independent &= message.parts.iter().all(|part| {
                                    prefix.explicit_committed_parts.contains(&part.part_id)
                                });
                            }
                            // Only an exact retained canonical prefix may
                            // repeat old identities. Inspect all new parts.
                            strip_checkpoint_baseline(&mut record, baseline.len())?;
                        }
                    }
                }
            }
            independent &= prefix.visit(&record, true);
        }
        Ok(())
    })?;
    Ok(if independent {
        conversation_appended
    } else {
        None
    })
}

fn strip_checkpoint_baseline(record: &mut JournalRecord, retained: usize) -> Result<()> {
    if let JournalEntry::HistoryMutated(mutation) = &mut record.entry
        && mutation.mutation == HistoryMutationKind::Checkpoint
    {
        anyhow::ensure!(
            mutation.messages.len() >= retained,
            "checkpoint prefix changed between snapshot passes"
        );
        // Payloads are already hydrated one record at a time. Drop the retained
        // canonical prefix before rendering; never clone it into the suffix.
        mutation.messages.drain(..retained);
    }
    Ok(())
}

/// Prefix-only identities. New identities are deliberately not inserted during
/// suffix inspection: repeats and forward references within one new turn need
/// the complete suffix renderer, but cannot affect previously cached rows.
#[derive(Default)]
struct PrefixIdentities {
    turns: HashSet<TurnId>,
    exchanges: HashSet<ExchangeId>,
    messages: HashSet<MessageId>,
    parts: HashSet<PartId>,
    explicit_committed_parts: HashSet<PartId>,
    calls: HashSet<CallId>,
}

impl PrefixIdentities {
    fn visit(&mut self, record: &JournalRecord, suffix: bool) -> bool {
        let mut independent = true;
        if let Some(turn) = record.turn_id {
            independent &= identity(&mut self.turns, turn, suffix);
        }
        match &record.entry {
            JournalEntry::HistoryMutated(mutation) => {
                for message in &mutation.messages {
                    if !suffix {
                        self.explicit_committed_parts
                            .extend(message.parts.iter().map(|part| part.part_id));
                    }
                    independent &= self.message(message, suffix);
                }
            }
            // Even invisible, uncommitted or hidden model messages determine
            // dates and may become visible when a later record commits a part.
            JournalEntry::ModelMessageRecorded(item) => {
                independent &= identity(&mut self.exchanges, item.exchange_id, suffix);
                independent &= self.message(&item.message, suffix);
            }
            JournalEntry::ModelResponseRecorded(item) => {
                independent &= identity(&mut self.exchanges, item.exchange_id, suffix);
                if let ModelResponseOutcome::Response { response } = &item.outcome {
                    for message in &response.messages {
                        independent &= self.message(message, suffix);
                    }
                    for call in &response.tool_calls {
                        independent &= identity(&mut self.calls, call.id.clone(), suffix);
                    }
                }
            }
            JournalEntry::SubagentSessionBoundRecorded(binding) => {
                independent &= identity(&mut self.calls, binding.source_call_id.clone(), suffix);
            }
            JournalEntry::ToolCallRecorded(item) => {
                independent &= identity(&mut self.calls, item.call.id.clone(), suffix);
            }
            JournalEntry::ToolResultRecorded(item) | JournalEntry::ToolEffectRecorded(item) => {
                independent &= identity(&mut self.calls, item.result.call_id.clone(), suffix);
            }
            _ => {}
        }
        independent
    }

    fn message(&mut self, message: &CanonicalMessage, suffix: bool) -> bool {
        let mut independent = identity(&mut self.messages, message.id, suffix);
        // Include every part, regardless of scope or provenance. Hidden parts
        // can be rehosted and still determine the first durable occurrence.
        for part in &message.parts {
            independent &= identity(&mut self.parts, part.part_id, suffix);
            match &part.payload {
                ContentPart::ToolCall { call } => {
                    independent &= identity(&mut self.calls, call.id.clone(), suffix);
                }
                ContentPart::ToolResult { result } => {
                    independent &= identity(&mut self.calls, result.call_id.clone(), suffix);
                }
                ContentPart::HostedToolActivity { activity } => {
                    independent &= identity(&mut self.calls, activity.id().to_owned(), suffix);
                }
                // Citation rendering searches backwards by tool name, without
                // an explicit identity; it could update an old hosted card.
                ContentPart::Citation { .. } if suffix => independent = false,
                _ => {}
            }
        }
        independent
    }
}

fn identity<T: Eq + std::hash::Hash>(set: &mut HashSet<T>, id: T, suffix: bool) -> bool {
    if suffix {
        !set.contains(&id)
    } else {
        set.insert(id);
        true
    }
}

#[cfg(test)]
pub(super) mod tests;
