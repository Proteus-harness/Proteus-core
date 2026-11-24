//! Private cold-start row owner for one certified text-only append.
//! Do not change AppTranscriptMessage or the persisted page wire format.
use proteus_contracts::{
    app_protocol::AppTranscriptMessage, domain::MessageId, model_standard::MessagePhase,
};
use serde::{Serialize, Serializer};

pub(crate) enum DisplayRows {
    Full(Vec<AppTranscriptMessage>),
    Text(Vec<CompactTextRow>),
}

impl DisplayRows {
    pub(crate) fn into_full(self) -> Vec<AppTranscriptMessage> {
        match self {
            Self::Full(rows) => rows,
            Self::Text(rows) => {
                let mut full = Vec::with_capacity(rows.len());
                full.extend(rows.into_iter().map(CompactTextRow::into_full));
                full
            }
        }
    }
}

pub(crate) struct CompactTextRow {
    pub(crate) message_id: MessageId,
    pub(crate) timestamp_ms: Option<i64>,
    pub(crate) phase: Option<MessagePhase>,
    pub(crate) role: &'static str,
    pub(crate) text: String,
}

impl CompactTextRow {
    fn into_full(self) -> AppTranscriptMessage {
        AppTranscriptMessage {
            message_id: Some(self.message_id),
            timestamp_ms: self.timestamp_ms,
            phase: self.phase,
            role: self.role.to_owned(),
            text: self.text,
            images: Vec::new(),
            tool: None,
            subagent: None,
            streaming: false,
        }
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

impl Serialize for CompactTextRow {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        TextWire {
            message_id: Some(self.message_id),
            timestamp_ms: self.timestamp_ms,
            phase: self.phase,
            role: self.role,
            text: &self.text,
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
    fn compact_wire_and_owned_fallback_match_the_public_row() {
        for (role, phase, timestamp_ms) in [
            ("user", None, None),
            ("assistant", Some(MessagePhase::FinalAnswer), Some(-8)),
            ("system", Some(MessagePhase::Commentary), Some(234)),
        ] {
            let row = CompactTextRow {
                message_id: MessageId::nil(),
                timestamp_ms,
                phase,
                role,
                text: "line\\n𝄞🦀\\u{0}\\\\".repeat(97),
            };
            let owner = row.text.as_ptr();
            let compact = serde_json::to_vec(&row).unwrap();
            let full = DisplayRows::Text(vec![row]).into_full();
            assert_eq!(full[0].text.as_ptr(), owner);
            assert_eq!(compact, serde_json::to_vec(&full[0]).unwrap());
        }
    }
}
