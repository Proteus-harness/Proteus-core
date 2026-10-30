//! Temporary identity index: dates come from the first durable occurrence,
//! including retained parts rehosted by a later replacement or compaction.
use std::{collections::HashMap, hash::Hash, num::NonZeroU64};

use crate::{
    core::{JournalEntry, JournalRecord, ModelResponseOutcome},
    domain::{CallId, MessageId, PartId},
    model_standard::{CanonicalMessage, ContentPart},
};

#[cfg(test)]
use crate::core::JournalProjection;

#[derive(Clone, Copy, Default)]
struct RecordedAt {
    // None is an explicitly vacant requested slot, not a guessed date. Validated
    // journal sequences start at one; Option<NonZeroU64> keeps a one-word key.
    sequence: Option<NonZeroU64>,
    timestamp_ms: i64,
}

#[derive(Default)]
pub(super) struct TranscriptTimestamps {
    messages: HashMap<MessageId, RecordedAt>,
    parts: HashMap<PartId, RecordedAt>,
    calls: HashMap<CallId, RecordedAt>,
}

impl TranscriptTimestamps {
    #[cfg(test)]
    pub(super) fn from_projection(projection: &JournalProjection) -> Self {
        Self::for_records(&projection.records, 0)
    }

    #[cfg(test)]
    pub(super) fn for_replay(projection: &JournalProjection, start: usize) -> Self {
        Self::for_records(&projection.records, start)
    }

    /// Restrict message/part keys to the replay suffix, but find their dates in
    /// the entire journal. Seed the same maps that will hold dates: no separate
    /// full-size wanted set or retained record payloads.
    pub(super) fn for_records(records: &[JournalRecord], start: usize) -> Self {
        if start == 0 {
            return Self::default().collect::<false>(records);
        }
        let mut index = Self::default();
        for record in &records[start..] {
            match &record.entry {
                JournalEntry::HistoryMutated(mutation) => {
                    for message in &mutation.messages {
                        index.require_message(message);
                    }
                }
                JournalEntry::ModelResponseRecorded(item) => {
                    if let ModelResponseOutcome::Response { response } = &item.outcome {
                        for message in &response.messages {
                            index.require_message(message);
                        }
                    }
                }
                _ => {}
            }
        }
        index.collect::<true>(records)
    }

    fn require_message(&mut self, message: &CanonicalMessage) {
        self.messages.entry(message.id).or_default();
        // Dates use all original parts, including hidden and duplicate parts.
        for part in &message.parts {
            self.parts.entry(part.part_id).or_default();
        }
    }

    fn collect<const SCOPED: bool>(mut self, records: &[JournalRecord]) -> Self {
        for record in records {
            let at = RecordedAt {
                sequence: Some(
                    NonZeroU64::new(record.session_seq)
                        .expect("validated journal sequences start at one"),
                ),
                timestamp_ms: record.timestamp_ms,
            };
            match &record.entry {
                JournalEntry::HistoryMutated(mutation) => {
                    for message in &mutation.messages {
                        self.record_message::<SCOPED>(message, at);
                    }
                }
                JournalEntry::ModelMessageRecorded(item) => {
                    self.record_message::<SCOPED>(&item.message, at);
                }
                JournalEntry::ModelResponseRecorded(item) => {
                    if let ModelResponseOutcome::Response { response } = &item.outcome {
                        for message in &response.messages {
                            self.record_message::<SCOPED>(message, at);
                        }
                    }
                }
                JournalEntry::ToolCallRecorded(item) => {
                    self.calls.entry(item.call.id.clone()).or_insert(at);
                }
                JournalEntry::ToolResultRecorded(item) | JournalEntry::ToolEffectRecorded(item) => {
                    self.calls.entry(item.result.call_id.clone()).or_insert(at);
                }
                _ => {}
            }
        }
        self
    }

    fn record_message<const SCOPED: bool>(&mut self, message: &CanonicalMessage, at: RecordedAt) {
        record_date::<SCOPED, _>(&mut self.messages, message.id, at);
        for part in &message.parts {
            record_date::<SCOPED, _>(&mut self.parts, part.part_id, at);
            let call_id = match &part.payload {
                ContentPart::ToolCall { call } => Some(&call.id),
                ContentPart::ToolResult { result } => Some(&result.call_id),
                _ => None,
            };
            if let Some(id) = call_id {
                // Hosted cards can reuse an old call ID. Keep call dates global
                // instead of adding an unbounded set of absent hosted-call IDs.
                self.calls.entry(id.clone()).or_insert(at);
            }
        }
    }

    pub(super) fn message(&self, message: &CanonicalMessage) -> Option<i64> {
        self.messages
            .get(&message.id)
            .into_iter()
            .chain(
                message
                    .parts
                    .iter()
                    .filter_map(|part| self.parts.get(&part.part_id)),
            )
            .filter(|at| at.sequence.is_some())
            // Journal order, not wall-clock ordering: a clock adjustment must
            // not move an existing message's creation time to a later record.
            .min_by_key(|at| at.sequence)
            .map(|at| at.timestamp_ms)
    }

    pub(super) fn call(&self, id: &str) -> Option<i64> {
        self.calls.get(id).map(|at| at.timestamp_ms)
    }
}

fn record_date<const SCOPED: bool, K: Eq + Hash>(
    dates: &mut HashMap<K, RecordedAt>,
    key: K,
    at: RecordedAt,
) {
    if SCOPED {
        if let Some(slot) = dates.get_mut(&key)
            && slot.sequence.is_none()
        {
            *slot = at;
        }
    } else {
        dates.entry(key).or_insert(at);
    }
}

#[cfg(test)]
mod tests;
