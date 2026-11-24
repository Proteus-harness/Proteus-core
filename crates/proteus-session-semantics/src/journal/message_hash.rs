//! Stable message comparison without allocating a serialized message on the heap.
use crate::model_standard::CanonicalMessage;
use anyhow::{Context, Result};
use ring::digest::SHA256;

pub fn history_message_sha256(message: &CanonicalMessage) -> Result<[u8; 32]> {
    const STACK_BYTES: usize = 1024;
    // Reserve half for the canonical envelope. A raw text byte can expand
    // into six JSON bytes, so even an all-control string need not be pre-scanned.
    const MAX_TEXT_BYTES: usize = STACK_BYTES / 2 / 6;
    // Plain singleton rows dominate many stored conversations. Serialize just
    // these small rows in a fixed stack slice and hash their bytes once. The
    // normal streaming path remains the authority for long or complex rows.
    if let [part] = message.parts.as_slice()
        && let crate::model_standard::ContentPart::Text { text } = &part.payload
        && text.len() <= MAX_TEXT_BYTES
        && message.metadata.is_null()
        && message.name.is_none()
        && message.tool_call_id.is_none()
    {
        let mut bytes = [0; STACK_BYTES];
        let mut tail = &mut bytes[..];
        if serde_json::to_writer(&mut tail, message).is_ok() {
            let used = STACK_BYTES - tail.len();
            return Ok(ring::digest::digest(&SHA256, &bytes[..used])
                .as_ref()
                .try_into()
                .expect("SHA256 digest length"));
        }
        // If the canonical envelope grows, discard the partial encoding.
    }
    let mut writer = HashWriter(ring::digest::Context::new(&SHA256));
    serde_json::to_writer(&mut writer, message)
        .context("failed to hash canonical history message")?;
    Ok(writer
        .0
        .finish()
        .as_ref()
        .try_into()
        .expect("SHA256 digest length"))
}

struct HashWriter(ring::digest::Context);

impl std::io::Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_standard::MessageRole;

    #[test]
    fn short_and_fallback_rows_match_the_exact_serialized_bytes() {
        for size in [0, 1, 63, 64, 65, 85, 86, 255, 256, 257, 511, 512, 513, 8192] {
            for unit in ["x", "\u{0}", "🚀", "\\", "\""] {
                let mut message = CanonicalMessage::text(MessageRole::User, unit.repeat(size));
                for add_metadata in [false, true] {
                    if add_metadata {
                        message.metadata = serde_json::json!({"z": ["\u{0}", 3], "a": -0.0});
                    }
                    let serialized = serde_json::to_vec(&message).unwrap();
                    assert_eq!(
                        &history_message_sha256(&message).unwrap(),
                        ring::digest::digest(&SHA256, &serialized).as_ref()
                    );
                }
            }
        }
    }
}
