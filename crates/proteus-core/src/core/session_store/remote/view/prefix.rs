//! Compact identities required to certify an append to a published transcript.
use crate::{
    core::{JournalEntry, JournalRecord, ModelResponseOutcome},
    domain::{CallId, ExchangeId, MessageId, PartId, TurnId},
    model_standard::{CanonicalMessage, ContentPart},
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

mod packed;

/// Prefix-only identities. New identities are deliberately not inserted during
/// suffix inspection: repeats and forward references within one new turn need
/// the complete suffix renderer, but cannot affect previously cached rows.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrefixIdentities {
    pub(super) history: Vec<[u8; 32]>,
    turns: HashSet<TurnId>,
    exchanges: HashSet<ExchangeId>,
    messages: HashSet<MessageId>,
    parts: HashSet<PartId>,
    pub(crate) explicit_committed_parts: HashSet<PartId>,
    calls: HashSet<CallId>,
}

impl PrefixIdentities {
    pub(crate) fn visit(&mut self, record: &JournalRecord, suffix: bool) -> bool {
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
