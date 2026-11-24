//! Stable content fingerprints without a serialized copy of a cached value.
use anyhow::{Context, Result};
use ring::digest::{Context as DigestContext, SHA256};
use serde::{
    Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq},
};
use serde_json::Value;

/// SHA-256 of compact JSON with lexicographically ordered object keys. Ordering
/// is explicit so independent worker builds agree even with `preserve_order`.
pub fn storage_value_etag(value: &Value) -> Result<String> {
    let mut writer = DigestWriter(DigestContext::new(&SHA256));
    serde_json::to_writer(&mut writer, &OrderedValue(value))
        .context("failed to fingerprint storage value")?;
    let digest = writer.0.finish();
    let mut etag = String::with_capacity(digest.as_ref().len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &byte in digest.as_ref() {
        etag.push(char::from(HEX[usize::from(byte >> 4)]));
        etag.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(etag)
}

struct OrderedValue<'a>(&'a Value);
impl Serialize for OrderedValue<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Array(values) => {
                let mut sequence = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    sequence.serialize_element(&OrderedValue(value))?;
                }
                sequence.end()
            }
            Value::Object(values) => {
                let mut map = serializer.serialize_map(Some(values.len()))?;
                // The default map and already canonical insertion-ordered
                // objects can both emit keys directly. Other objects still
                // require an independent lexicographic sort.
                if can_write_in_order(values) {
                    for (key, value) in values {
                        map.serialize_entry(key, &OrderedValue(value))?;
                    }
                } else {
                    let mut entries = values.iter().collect::<Vec<_>>();
                    entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
                    for (key, value) in entries {
                        map.serialize_entry(key, &OrderedValue(value))?;
                    }
                }
                map.end()
            }
            value => value.serialize(serializer),
        }
    }
}

fn can_write_in_order(values: &serde_json::Map<String, Value>) -> bool {
    // Keep speculative comparisons bounded even if an almost-sorted object has
    // its only inversion at the end or adversarially long common key prefixes.
    const MAX_KEYS: usize = 32;
    const MAX_KEY_BYTES: usize = 128;
    if values.len() > MAX_KEYS {
        return false;
    }
    let mut previous: Option<&str> = None;
    for key in values.keys() {
        if key.len() > MAX_KEY_BYTES || previous.is_some_and(|p| p >= key.as_str()) {
            return false;
        }
        previous = Some(key);
    }
    true
}

struct DigestWriter(DigestContext);
impl std::io::Write for DigestWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
