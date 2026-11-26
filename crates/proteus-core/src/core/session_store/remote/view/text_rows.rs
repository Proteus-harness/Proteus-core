//! Private cold-start row owner for one certified text-only append.
//! Do not change AppTranscriptMessage or the persisted page wire format.
use proteus_contracts::{
    app_protocol::AppTranscriptMessage,
    domain::MessageId,
    model_standard::{MessagePhase, MessageRole},
};
use serde::{Serialize, Serializer};

pub(crate) enum DisplayRows {
    Full(Vec<AppTranscriptMessage>),
    Text {
        // The only producer proves one physical all-text Append.
        timestamp_ms: i64,
        rows: Vec<CompactTextRow>,
    },
}

impl DisplayRows {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Full(rows) => rows.len(),
            Self::Text { rows, .. } => rows.len(),
        }
    }

    pub(super) fn copy_range(&self, range: std::ops::Range<usize>) -> Vec<AppTranscriptMessage> {
        match self {
            Self::Full(rows) => rows[range].to_vec(),
            Self::Text { rows, timestamp_ms } => rows[range]
                .iter()
                .cloned()
                .map(|row| row.into_full(*timestamp_ms))
                .collect(),
        }
    }

    pub(crate) fn into_full(self) -> Vec<AppTranscriptMessage> {
        match self {
            Self::Full(rows) => rows,
            Self::Text { timestamp_ms, rows } => {
                let mut full = Vec::with_capacity(rows.len());
                full.extend(rows.into_iter().map(|row| row.into_full(timestamp_ms)));
                full
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct CompactTextRow {
    pub(crate) message_id: MessageId,
    pub(crate) phase: Option<MessagePhase>,
    pub(crate) role: MessageRole,
    pub(crate) text: String,
}

impl CompactTextRow {
    pub(super) fn wire(&self, timestamp_ms: i64) -> impl Serialize + '_ {
        TextRowWithDate {
            row: self,
            timestamp_ms,
        }
    }

    fn into_full(self, timestamp_ms: i64) -> AppTranscriptMessage {
        AppTranscriptMessage {
            message_id: Some(self.message_id),
            timestamp_ms: Some(timestamp_ms),
            phase: self.phase,
            role: transcript_role(&self.role).to_owned(),
            text: self.text,
            images: Vec::new(),
            tool: None,
            subagent: None,
            streaming: false,
        }
    }
}

pub(crate) fn transcript_role(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System | MessageRole::Developer => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "system",
        _ => "system",
    }
}

// Match AppTranscriptMessage's declaration order even in preserve_order.
#[derive(Serialize)]
struct TextWire<'a> {
    message_id: Option<MessageId>,
    timestamp_ms: Option<i64>,
    phase: Option<MessagePhase>,
    role: &'a str,
    text: &'a str,
    images: [(); 0],
    tool: Option<()>,
    subagent: Option<()>,
    streaming: bool,
}

// Borrowed two-word item; only a page of these is retained by the writer.
struct TextRowWithDate<'a> {
    row: &'a CompactTextRow,
    timestamp_ms: i64,
}

impl Serialize for TextRowWithDate<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        TextWire {
            message_id: Some(self.row.message_id),
            timestamp_ms: Some(self.timestamp_ms),
            phase: self.row.phase,
            role: transcript_role(&self.row.role),
            text: &self.row.text,
            images: [],
            tool: None,
            subagent: None,
            streaming: false,
        }
        .serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_text_keeps_role_phase_signed_date_and_fallback_wire() {
        for (role, expected_role, phase, timestamp_ms) in [
            (MessageRole::User, "user", None, i64::MIN),
            (
                MessageRole::Assistant,
                "assistant",
                Some(MessagePhase::FinalAnswer),
                i64::MAX,
            ),
            (
                MessageRole::System,
                "system",
                Some(MessagePhase::Commentary),
                -8,
            ),
            (MessageRole::Developer, "system", None, 0),
            (MessageRole::Tool, "system", None, 234),
        ] {
            let row = CompactTextRow {
                message_id: MessageId::nil(),
                phase,
                role,
                text: "line\\n𝄞🦀\\u{0}\\\\".repeat(97),
            };
            let text_owner = row.text.as_ptr();
            let compact = serde_json::to_vec(&row.wire(timestamp_ms)).unwrap();
            let full = DisplayRows::Text {
                timestamp_ms,
                rows: vec![row],
            }
            .into_full();
            assert_eq!(full[0].text.as_ptr(), text_owner);
            assert_eq!(full[0].role, expected_role);
            assert_eq!(full[0].timestamp_ms, Some(timestamp_ms));
            assert_eq!(full[0].phase, phase);
            assert_eq!(compact, serde_json::to_vec(&full[0]).unwrap());
        }
    }
}
