//! Temporary identity index: dates come from the first durable occurrence,
//! including retained parts rehosted by a later replacement or compaction.
use std::collections::HashMap;

use crate::{
    core::{JournalEntry, JournalProjection, ModelResponseOutcome},
    domain::{CallId, MessageId, PartId},
    model_standard::{CanonicalMessage, ContentPart},
};

#[derive(Clone, Copy)]
struct RecordedAt {
    sequence: u64,
    timestamp_ms: i64,
}

#[derive(Default)]
pub(super) struct TranscriptTimestamps {
    messages: HashMap<MessageId, RecordedAt>,
    parts: HashMap<PartId, RecordedAt>,
    calls: HashMap<CallId, RecordedAt>,
}

impl TranscriptTimestamps {
    pub(super) fn from_projection(projection: &JournalProjection) -> Self {
        let mut index = Self::default();
        for record in &projection.records {
            let at = RecordedAt {
                sequence: record.session_seq,
                timestamp_ms: record.timestamp_ms,
            };
            match &record.entry {
                JournalEntry::HistoryMutated(mutation) => {
                    for message in &mutation.messages {
                        index.record_message(message, at);
                    }
                }
                JournalEntry::ModelMessageRecorded(item) => index.record_message(&item.message, at),
                JournalEntry::ModelResponseRecorded(item) => {
                    if let ModelResponseOutcome::Response { response } = &item.outcome {
                        for message in &response.messages {
                            index.record_message(message, at);
                        }
                    }
                }
                JournalEntry::ToolCallRecorded(item) => {
                    index.calls.entry(item.call.id.clone()).or_insert(at);
                }
                JournalEntry::ToolResultRecorded(item) | JournalEntry::ToolEffectRecorded(item) => {
                    index.calls.entry(item.result.call_id.clone()).or_insert(at);
                }
                _ => {}
            }
        }
        index
    }

    fn record_message(&mut self, message: &CanonicalMessage, at: RecordedAt) {
        self.messages.entry(message.id).or_insert(at);
        for part in &message.parts {
            self.parts.entry(part.part_id).or_insert(at);
            let call_id = match &part.payload {
                ContentPart::ToolCall { call } => Some(&call.id),
                ContentPart::ToolResult { result } => Some(&result.call_id),
                _ => None,
            };
            if let Some(id) = call_id {
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
            // Journal order, not wall-clock ordering: a clock adjustment must
            // not move an existing message's creation time to a later record.
            .min_by_key(|at| at.sequence)
            .map(|at| at.timestamp_ms)
    }

    pub(super) fn call(&self, id: &str) -> Option<i64> {
        self.calls.get(id).map(|at| at.timestamp_ms)
    }
}
