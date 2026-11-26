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
    let mut writer = HashWriter::new();
    serde_json::to_writer(&mut writer, message)
        .context("failed to hash canonical history message")?;
    Ok(writer
        .finish()
        .as_ref()
        .try_into()
        .expect("SHA256 digest length"))
}

const HASH_BUFFER_BYTES: usize = 1024;

struct HashWriter {
    digest: ring::digest::Context,
    pending: [u8; HASH_BUFFER_BYTES],
    used: usize,
}

impl HashWriter {
    fn new() -> Self {
        Self {
            digest: ring::digest::Context::new(&SHA256),
            pending: [0; HASH_BUFFER_BYTES],
            used: 0,
        }
    }

    fn flush_pending(&mut self) {
        if self.used != 0 {
            self.digest.update(&self.pending[..self.used]);
            self.used = 0;
        }
    }

    fn finish(mut self) -> ring::digest::Digest {
        self.flush_pending();
        self.digest.finish()
    }
}

impl std::io::Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        // Coalesce small serde fragments. Feed large text as-is after older
        // fragments, without a full copy or any heap allocation.
        if bytes.len() >= HASH_BUFFER_BYTES {
            self.flush_pending();
            self.digest.update(bytes);
        } else {
            if bytes.len() > HASH_BUFFER_BYTES - self.used {
                self.flush_pending();
            }
            let end = self.used + bytes.len();
            self.pending[self.used..end].copy_from_slice(bytes);
            self.used = end;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.flush_pending();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_standard::MessageRole;

    #[test]
    fn buffer_matches_canonical_fixture_rows_and_coalesces_serde_fragments() {
        use std::io::Write;

        struct Fragments {
            inner: HashWriter,
            writes: usize,
        }

        impl Write for Fragments {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.writes += usize::from(!bytes.is_empty());
                self.inner.write(bytes)
            }

            fn flush(&mut self) -> std::io::Result<()> {
                self.inner.flush()
            }
        }

        // The endpoints of the checked 50k history-scale input (four repeats).
        for i in [0, 49_999] {
            let role = if i % 2 == 0 {
                MessageRole::User
            } else {
                MessageRole::Assistant
            };
            let text = format!(
                "Fixture row {i}\n{}",
                "Синтетический текст для замера памяти и прокрутки. ".repeat(4)
            );
            assert!(text.len() > 85);
            let message = CanonicalMessage::text(role, text);
            let expected = serde_json::to_vec(&message).unwrap();
            assert!(expected.len() < HASH_BUFFER_BYTES);
            let mut writer = Fragments {
                inner: HashWriter::new(),
                writes: 0,
            };
            serde_json::to_writer(&mut writer, &message).unwrap();
            // The old digest path fed each of these separately. The exact same
            // complete output now fits in one pending digest update.
            assert!(writer.writes > 4);
            assert_eq!(writer.inner.used, expected.len());
            let digest = ring::digest::digest(&SHA256, &expected);
            assert_eq!(writer.inner.finish().as_ref(), digest.as_ref());
            assert_eq!(&history_message_sha256(&message).unwrap(), digest.as_ref());
        }
    }

    #[test]
    fn digest_writer_preserves_split_order_and_flush_at_buffer_boundaries() {
        use std::io::Write;

        let first = [b'a'; HASH_BUFFER_BYTES - 1];
        let direct_equal = [b'd'; HASH_BUFFER_BYTES];
        let direct_larger = [b'e'; HASH_BUFFER_BYTES + 11];
        let mut writer = HashWriter::new();
        let mut expected = Vec::new();
        for (chunk, pending) in [
            (&first[..], HASH_BUFFER_BYTES - 1),
            (&b"b"[..], HASH_BUFFER_BYTES),
            (&b""[..], HASH_BUFFER_BYTES),
            (&b"c"[..], 1),
            (&direct_equal[..], 0),
            (&direct_larger[..], 0),
            (&b"tail"[..], 4),
        ] {
            assert_eq!(writer.write(chunk).unwrap(), chunk.len());
            assert_eq!(writer.used, pending);
            expected.extend_from_slice(chunk);
        }
        writer.flush().unwrap();
        writer.flush().unwrap();
        assert_eq!(writer.used, 0);
        writer.write_all(b"after flush").unwrap();
        expected.extend_from_slice(b"after flush");
        assert_eq!(
            writer.finish().as_ref(),
            ring::digest::digest(&SHA256, &expected).as_ref()
        );
    }

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
