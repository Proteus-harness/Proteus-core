//! Stable content fingerprints without a serialized copy of a cached value.
use std::io;

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
    fingerprint::<false>(value, 0).map(|(etag, _)| etag)
}

/// The same canonical etag and compact JSON byte count, with an inclusive
/// serialized byte limit. No partial etag is returned on overflow.
pub fn storage_value_etag_with_len(value: &Value, max_bytes: usize) -> Result<(String, usize)> {
    fingerprint::<true>(value, max_bytes)
}

fn fingerprint<const BOUNDED: bool>(value: &Value, limit: usize) -> Result<(String, usize)> {
    let mut writer = DigestWriter::<BOUNDED> {
        digest: DigestContext::new(&SHA256),
        remaining: limit,
    };
    serde_json::to_writer(&mut writer, &OrderedValue(value))
        .context("failed to fingerprint storage value")?;
    let digest = writer.digest.finish();
    let mut etag = String::with_capacity(digest.as_ref().len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &byte in digest.as_ref() {
        etag.push(char::from(HEX[usize::from(byte >> 4)]));
        etag.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok((etag, limit - writer.remaining))
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
                } else if values.len() <= SMALL_OBJECT_KEYS {
                    return write_small_object(map, values);
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

const SMALL_OBJECT_KEYS: usize = 12;

// Keep scratch out of the serializer's frames for arrays and sorted objects.
#[inline(never)]
fn write_small_object<M: SerializeMap>(
    mut map: M,
    values: &serde_json::Map<String, Value>,
) -> Result<M::Ok, M::Error> {
    let mut iter = values.iter();
    let first = iter.next().expect("unsorted object has entries");
    let mut scratch = [first; SMALL_OBJECT_KEYS];
    let entries = &mut scratch[..values.len()];
    for (slot, entry) in entries[1..].iter_mut().zip(iter) {
        *slot = entry;
    }
    entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    for &(key, value) in entries.iter() {
        map.serialize_entry(key, &OrderedValue(value))?;
    }
    map.end()
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

struct DigestWriter<const BOUNDED: bool> {
    digest: DigestContext,
    remaining: usize,
}

impl<const BOUNDED: bool> io::Write for DigestWriter<BOUNDED> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if BOUNDED {
            self.remaining = self
                .remaining
                .checked_sub(bytes.len())
                .ok_or_else(|| io::Error::other("encoded JSON exceeds byte budget"))?;
        }
        self.digest.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, json};

    #[test]
    fn bounded_storage_fingerprint_matches_canonical_with_exact_json_cap() {
        let nested = Map::from_iter([
            ("z".into(), json!(["\n\t\\\0", "ключ🦀", -1.5, null])),
            ("a".into(), json!({"second": 2, "first": true})),
        ]);
        for value in [
            json!(null),
            json!(""),
            json!([Value::Object(nested), false]),
            json!({"deflate_v1": ["AA==", "BB==", "CC=="]}),
        ] {
            let bytes = serde_json::to_vec(&value).unwrap().len();
            let expected = storage_value_etag(&value).unwrap();
            assert_eq!(
                storage_value_etag_with_len(&value, bytes).unwrap(),
                (expected.clone(), bytes)
            );
            assert_eq!(
                storage_value_etag_with_len(&value, bytes + 1).unwrap(),
                (expected, bytes)
            );
            assert!(storage_value_etag_with_len(&value, bytes - 1).is_err());
            assert!(storage_value_etag_with_len(&value, 0).is_err());
        }
    }
}
