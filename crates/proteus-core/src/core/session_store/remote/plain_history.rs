//! The existing #665 plain wire row shared by two independent owners.
//! Staged display and validated canonical history must never substitute data
//! for one another. Canonical serializers keep all original fields/key order.
use crate::{
    domain::{MessageId, PartId},
    model_standard::{
        CanonicalMessage, ContentPart, MessagePhase, MessageRole, PartProvenance, PartScope,
    },
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{Error, Visitor},
};
use std::fmt;

// Match only JSON null even if a future caller uses a deserializer whose
// deserialize_unit accepts other shapes (Serde Content can accept an empty map).
pub(super) struct OnlyNull;

impl<'de> Deserialize<'de> for OnlyNull {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Null;
        impl<'de> Visitor<'de> for Null {
            type Value = OnlyNull;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("null")
            }

            fn visit_unit<E: Error>(self) -> Result<OnlyNull, E> {
                Ok(OnlyNull)
            }
        }
        deserializer.deserialize_any(Null)
    }
}

impl Serialize for OnlyNull {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_unit()
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PlainHistoryMessage {
    id: MessageId,
    role: MessageRole,
    phase: Option<MessagePhase>,
    parts: [Part; 1],
    #[serde(rename = "name")]
    _name: Option<OnlyNull>,
    #[serde(rename = "tool_call_id")]
    _tool_call_id: Option<OnlyNull>,
    #[serde(rename = "metadata")]
    _metadata: OnlyNull,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Part {
    part_id: PartId,
    #[serde(rename = "provenance")]
    _provenance: PartProvenance,
    scope: PartScope,
    payload: Payload,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
enum Payload {
    Text { text: String },
}

impl PlainHistoryMessage {
    pub(super) fn accepts(message: &CanonicalMessage) -> bool {
        message.name.is_none()
            && message.tool_call_id.is_none()
            && message.metadata.is_null()
            && matches!(message.parts.as_slice(),
                [part] if matches!(part.payload, ContentPart::Text { .. })
            )
    }

    // Only called after the complete immutable shape preflight. The caller
    // drops the original shell, except for one reused typed digest scratch.
    pub(super) fn take_from(message: &mut CanonicalMessage) -> Self {
        debug_assert!(Self::accepts(message));
        let part = &mut message.parts[0];
        let ContentPart::Text { text } = &mut part.payload else {
            unreachable!("plain canonical preflight");
        };
        Self {
            id: message.id,
            role: message.role.clone(),
            phase: message.phase,
            parts: [Part {
                part_id: part.part_id,
                _provenance: part.provenance,
                scope: part.scope,
                payload: Payload::Text {
                    text: std::mem::take(text),
                },
            }],
            _name: None,
            _tool_call_id: None,
            _metadata: OnlyNull,
        }
    }

    pub(super) fn identities(&self) -> (MessageId, PartId) {
        (self.id, self.parts[0].part_id)
    }

    pub(super) fn role_and_text(&self) -> (&MessageRole, &str) {
        let Payload::Text { text } = &self.parts[0].payload;
        (&self.role, text)
    }

    pub(super) fn into_display_fields(
        self,
    ) -> (
        MessageId,
        MessageRole,
        Option<MessagePhase>,
        PartId,
        PartScope,
        String,
    ) {
        let [part] = self.parts;
        let Payload::Text { text } = part.payload;
        (
            self.id,
            self.role,
            self.phase,
            part.part_id,
            part.scope,
            text,
        )
    }

    pub(super) fn with_typed_digest_row<T>(
        &mut self,
        scratch: &mut CanonicalMessage,
        hash: impl FnOnce(&CanonicalMessage) -> T,
    ) -> T {
        // Scratch is one of the original shells: no RNG and no per-row Vec.
        scratch.id = self.id;
        scratch.role = self.role.clone();
        scratch.phase = self.phase;
        let part = &self.parts[0];
        scratch.parts[0].part_id = part.part_id;
        scratch.parts[0].scope = part.scope;
        scratch.parts[0].provenance = part._provenance;
        self.swap_text(scratch);
        let result = hash(scratch);
        self.swap_text(scratch);
        result
    }

    fn swap_text(&mut self, scratch: &mut CanonicalMessage) {
        let ContentPart::Text { text: scratch } = &mut scratch.parts[0].payload else {
            unreachable!("plain canonical digest scratch");
        };
        let Payload::Text { text } = &mut self.parts[0].payload;
        std::mem::swap(text, scratch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_wire_fingerprint_and_pointer_sharing_with_only_typed_scratch() {
        for (index, role) in [
            MessageRole::System,
            MessageRole::Developer,
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::Tool,
        ]
        .into_iter()
        .enumerate()
        {
            for size in [1, 85, 86, 4097] {
                let value = if size == 4097 {
                    "x🦀\\\"\u{0}".repeat(size)
                } else {
                    "x".repeat(size)
                };
                let mut original = CanonicalMessage::text(role.clone(), value);
                original.id = MessageId::from_u128(100 + index as u128);
                original.phase = Some(MessagePhase::FinalAnswer);
                original.parts[0].part_id = PartId::from_u128(200 + index as u128);
                original.parts[0].provenance = PartProvenance::Model;
                original.parts[0].scope = PartScope::Trace;
                let ContentPart::Text { text } = &original.parts[0].payload else {
                    unreachable!();
                };
                let pointer = text.as_ptr();
                let bytes = serde_json::to_vec(&original).unwrap();
                let value = serde_json::to_value(&original).unwrap();
                let exact = crate::core::history_message_sha256(&original).unwrap();
                let mut row = PlainHistoryMessage::take_from(&mut original);
                assert_eq!(serde_json::to_vec(&row).unwrap(), bytes);
                assert_eq!(serde_json::to_value(&row).unwrap(), value);
                assert_eq!(row.role_and_text().1.as_ptr(), pointer);
                // Deliberately start with a different typed shell. The digest
                // adapter must change more than the text.
                let mut scratch = CanonicalMessage::text(MessageRole::Developer, String::new());
                scratch.id = MessageId::from_u128(9000);
                scratch.parts[0].part_id = PartId::from_u128(9001);
                let got = row
                    .with_typed_digest_row(&mut scratch, crate::core::history_message_sha256)
                    .unwrap();
                assert_eq!(got, exact);
                let error: Result<(), _> = row.with_typed_digest_row(&mut scratch, |_| Err("test"));
                assert_eq!(error, Err("test"));
                assert_eq!(row.role_and_text().1.as_ptr(), pointer);
                assert_eq!(serde_json::to_vec(&row).unwrap(), bytes);
            }
        }
    }
}
