//! Compact identities required to certify an append to a published transcript.
use crate::{
    core::{JournalEntry, JournalRecord, ModelResponseOutcome},
    domain::{CallId, ExchangeId, MessageId, PartId, TurnId},
    model_standard::{CanonicalMessage, ContentPart},
};
use std::collections::{HashMap, HashSet};

mod packed;

/// Prefix-only identities. New identities are deliberately not inserted during
/// suffix inspection: repeats and forward references within one new turn need
/// the complete suffix renderer, but cannot affect previously cached rows.
#[derive(Debug, Default)]
pub(crate) struct PrefixIdentities {
    pub(super) history: Vec<[u8; 32]>,
    turns: HashSet<TurnId>,
    exchanges: HashSet<ExchangeId>,
    messages: HashSet<MessageId>,
    // Keep all observed parts; the bit records whether history ever committed one.
    parts: HashMap<PartId, bool>,
    calls: HashSet<CallId>,
}

impl PrefixIdentities {
    pub(crate) fn visit(&mut self, record: &JournalRecord, suffix: bool) -> bool {
        let mut independent = self.visit_turn(record.turn_id, suffix);
        match &record.entry {
            JournalEntry::HistoryMutated(mutation) => {
                for message in &mutation.messages {
                    independent &= self.visit_history_message(message, suffix);
                }
            }
            // Even invisible, uncommitted or hidden model messages determine
            // dates and may become visible when a later record commits a part.
            JournalEntry::ModelMessageRecorded(item) => {
                independent &= identity(&mut self.exchanges, item.exchange_id, suffix);
                independent &= self.message(&item.message, suffix, false);
            }
            JournalEntry::ModelResponseRecorded(item) => {
                independent &= identity(&mut self.exchanges, item.exchange_id, suffix);
                if let ModelResponseOutcome::Response { response } = &item.outcome {
                    for message in &response.messages {
                        independent &= self.message(message, suffix, false);
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

    pub(in crate::core::session_store::remote) fn visit_owned_prefix(
        &mut self,
        record: JournalRecord,
    ) {
        if let JournalEntry::HistoryMutated(mutation) = record.entry {
            self.visit_turn(record.turn_id, false);
            for message in mutation.messages {
                self.visit_history_message(&message, false);
                // No later prefix visit needs this row's decoded payload.
            }
        } else {
            self.visit(&record, false);
        }
    }

    // The narrow reader has a single Text part per history message. Include
    // all IDs, before visibility/blank filtering, as the canonical visit does.
    pub(in crate::core::session_store::remote) fn from_staged_text(
        turn: Option<TurnId>,
        rows: impl Iterator<Item = (MessageId, PartId)>,
    ) -> Self {
        let mut prefix = Self::default();
        prefix.visit_turn(turn, false);
        for (message, part) in rows {
            prefix.messages.insert(message);
            prefix.parts.insert(part, true);
        }
        prefix
    }

    fn visit_turn(&mut self, turn: Option<TurnId>, suffix: bool) -> bool {
        turn.is_none_or(|turn| identity(&mut self.turns, turn, suffix))
    }

    pub(crate) fn has_explicit_part(&self, id: &PartId) -> bool {
        self.parts.get(id).copied().unwrap_or(false)
    }

    fn visit_history_message(&mut self, message: &CanonicalMessage, suffix: bool) -> bool {
        self.message(message, suffix, true)
    }

    fn message(&mut self, message: &CanonicalMessage, suffix: bool, explicit: bool) -> bool {
        let mut independent = identity(&mut self.messages, message.id, suffix);
        // Include every part, regardless of scope or provenance. Hidden parts
        // can be rehosted and still determine the first durable occurrence.
        for part in &message.parts {
            if suffix {
                independent &= !self.parts.contains_key(&part.part_id);
            } else if explicit {
                self.parts.insert(part.part_id, true);
            } else {
                // A later model observation must not undo an earlier commit.
                self.parts.entry(part.part_id).or_insert(false);
            }
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
