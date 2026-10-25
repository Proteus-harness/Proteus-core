//! Versioned read-only projection, retaining the complete canonical snapshot.
use serde::{Deserialize, Serialize};

use super::ConversationSnapshot;
use crate::domain::SessionId;

pub const CONVERSATION_VIEW_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConversationView {
    pub schema_version: u32,
    pub source: ConversationViewSource,
    pub projection: ConversationViewProjection,
    pub conversation: ConversationSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationViewSource {
    pub harness: String,
    pub version: String,
    pub session_id: SessionId,
    /// Revision of this snapshot, not an SSE cursor or a journal sequence.
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationViewProjection {
    /// Current canonical history after compaction; excludes replaced history,
    /// the full execution journal and uncheckpointed streaming progress.
    Conversation,
}

impl ConversationView {
    pub fn new(
        session_id: SessionId,
        version: impl Into<String>,
        conversation: ConversationSnapshot,
    ) -> Self {
        Self {
            schema_version: CONVERSATION_VIEW_SCHEMA_VERSION,
            source: ConversationViewSource {
                harness: "proteus".to_owned(),
                version: version.into(),
                session_id,
                revision: conversation.revision,
            },
            projection: ConversationViewProjection::Conversation,
            conversation,
        }
    }
}

#[cfg(test)]
mod tests;
