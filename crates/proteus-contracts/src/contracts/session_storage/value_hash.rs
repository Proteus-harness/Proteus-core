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
    Ok(writer
        .0
        .finish()
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
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
                let mut entries = values.iter().collect::<Vec<_>>();
                entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    map.serialize_entry(key, &OrderedValue(value))?;
                }
                map.end()
            }
            value => value.serialize(serializer),
        }
    }
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
