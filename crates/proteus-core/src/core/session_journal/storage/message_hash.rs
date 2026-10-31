//! Stable message comparison without a second serialized message allocation.
use super::*;
use crate::model_standard::CanonicalMessage;

pub(crate) fn history_message_sha256(message: &CanonicalMessage) -> Result<[u8; 32]> {
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
