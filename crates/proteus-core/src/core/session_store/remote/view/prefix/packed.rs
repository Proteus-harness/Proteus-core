//! The append certificate stores exact keys, not lossy fingerprints of IDs.
//! Its outer cache envelope still validates the complete JSON value by SHA.
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use proteus_contracts::contracts::SESSION_STORAGE_MAX_CACHE_BYTES as LIMIT;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;
use uuid::Uuid;

// Only pack() can construct this writer-side value. The decoder uses
// Packed<C, String> instead and does not trust received fields.
#[derive(Serialize)]
#[serde(transparent)]
pub(in crate::core::session_store::remote::view) struct GeneratedBase64(String);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::session_store::remote::view) struct Packed<C, B = String> {
    format: u8,
    history: B,
    turns: B,
    exchanges: B,
    messages: B,
    // Each 17-byte record is UUID plus a strict 0/1 explicit membership flag.
    parts: B,
    calls: C,
}

pub(in crate::core::session_store::remote::view) type EncodedPrefix<'a> =
    Packed<Cow<'a, HashSet<CallId>>, GeneratedBase64>;

impl<C: Serialize> Packed<C, GeneratedBase64> {
    fn fits_bytes(&self, limit: usize) -> Option<()> {
        // The generated standard Base64 alphabet contains no JSON-escaped
        // characters. Charge its exact bytes without walking the strings.
        let limit = [
            &self.history,
            &self.turns,
            &self.exchanges,
            &self.messages,
            &self.parts,
        ]
        .into_iter()
        .try_fold(limit, |left, field| left.checked_sub(field.0.len()))?;
        // Count quotes, keys, punctuation and unrestricted call IDs with
        // the same wire schema and serializer, not a hand-written constant.
        let overhead = Packed {
            format: self.format,
            history: "",
            turns: "",
            exchanges: "",
            messages: "",
            parts: "",
            calls: &self.calls,
        };
        crate::core::session_store::json_size::encoded_len(&overhead, limit).map(|_| ())
    }

    pub(in crate::core::session_store::remote::view) fn into_bounded_value(self) -> Option<Value> {
        self.fits_bytes(LIMIT)?;
        // Match the derived field order, including when Map preserves insertion order.
        let mut object = serde_json::Map::with_capacity(7);
        object.insert("format".to_owned(), Value::from(self.format));
        object.insert("history".to_owned(), Value::String(self.history.0));
        object.insert("turns".to_owned(), Value::String(self.turns.0));
        object.insert("exchanges".to_owned(), Value::String(self.exchanges.0));
        object.insert("messages".to_owned(), Value::String(self.messages.0));
        object.insert("parts".to_owned(), Value::String(self.parts.0));
        object.insert("calls".to_owned(), serde_json::to_value(self.calls).ok()?);
        Some(Value::Object(object))
    }
}

fn pack<const N: usize>(
    records: impl Iterator<Item = [u8; N]>,
    n: usize,
) -> GeneratedBase64 {
    const BATCH_RECORDS: usize = 48;
    if n <= BATCH_RECORDS {
        let mut bytes = Vec::with_capacity(n * N);
        for item in records {
            bytes.extend_from_slice(&item);
        }
        return GeneratedBase64(STANDARD.encode(bytes));
    }

    let size = base64::encoded_len(n * N, true).expect("encoded prefix length");
    let mut output = vec![0; size];
    let mut written = 0;
    let mut encode = |batch: &[u8]| {
        let size = base64::encoded_len(batch.len(), true).expect("encoded prefix batch length");
        let end = written + size;
        if end > output.len() {
            output.resize(end, 0);
        }
        STANDARD
            .encode_slice(batch, &mut output[written..end])
            .expect("sized prefix batch output");
        written = end;
    };

    // Full batches contain a multiple of three bytes. Only the final
    // partial batch can add Base64 padding.
    let mut batch = [[0; N]; BATCH_RECORDS];
    let mut used = 0;
    for item in records {
        batch[used] = item;
        used += 1;
        if used == BATCH_RECORDS {
            encode(batch.as_flattened());
            used = 0;
        }
    }
    if used != 0 {
        encode(batch[..used].as_flattened());
    }
    output.truncate(written);
    GeneratedBase64(String::from_utf8(output).expect("Base64 is UTF-8"))
}

fn ids(set: &HashSet<Uuid>) -> GeneratedBase64 {
    pack(set.iter().map(|id| *id.as_bytes()), set.len())
}

fn unpack<const N: usize>(s: String, mut visit: impl FnMut([u8; N]) -> Option<()>) -> Option<()> {
    let raw = STANDARD.decode(s).ok()?;
    let chunks = raw.chunks_exact(N);
    if !chunks.remainder().is_empty() {
        return None;
    }
    for bytes in chunks {
        visit(bytes.try_into().ok()?)?;
    }
    Some(())
}

// A startup projection does not retain the certificate after cache publication.
// Lend the same exact input on ordinary/committed saves, or release the
// startup owners progressively as each fixed-format field is encoded.
struct PrefixFields<'a> {
    history: Cow<'a, [[u8; 32]]>,
    turns: Cow<'a, HashSet<TurnId>>,
    exchanges: Cow<'a, HashSet<ExchangeId>>,
    messages: Cow<'a, HashSet<MessageId>>,
    parts: Cow<'a, HashMap<PartId, bool>>,
    calls: Cow<'a, HashSet<CallId>>,
}

impl<'a> PrefixFields<'a> {
    fn encode(self) -> Option<EncodedPrefix<'a>> {
        let Self {
            history,
            turns,
            exchanges,
            messages,
            parts,
            calls,
        } = self;
        // Bound all binary allocations before creating them. The caller also
        // checks the actual serialized JSON byte count (including escapes).
        let mut left = LIMIT;
        for (count, width) in [
            (history.len(), 32),
            (turns.len(), 16),
            (exchanges.len(), 16),
            (messages.len(), 16),
            (parts.len(), 17),
        ] {
            left = left.checked_sub(base64::encoded_len(count.checked_mul(width)?, true)?)?;
        }
        for call in calls.iter() {
            left = left.checked_sub(call.len().checked_add(2)?)?;
        }
        let encoded_parts = pack(
            parts.iter().map(|(id, explicit)| {
                let mut record = [0; 17];
                record[..16].copy_from_slice(id.as_bytes());
                record[16] = u8::from(*explicit);
                record
            }),
            parts.len(),
        );
        drop(parts);
        let encoded_history = pack(history.iter().copied(), history.len());
        drop(history);
        let encoded_turns = ids(&turns);
        drop(turns);
        let encoded_exchanges = ids(&exchanges);
        drop(exchanges);
        let encoded_messages = ids(&messages);
        drop(messages);
        Some(Packed {
            format: 1,
            history: encoded_history,
            turns: encoded_turns,
            exchanges: encoded_exchanges,
            messages: encoded_messages,
            parts: encoded_parts,
            calls,
        })
    }
}

impl PrefixIdentities {
    pub(in crate::core::session_store::remote::view) fn encode_cache(
        &self,
    ) -> Option<EncodedPrefix<'_>> {
        PrefixFields {
            history: Cow::Borrowed(&self.history),
            turns: Cow::Borrowed(&self.turns),
            exchanges: Cow::Borrowed(&self.exchanges),
            messages: Cow::Borrowed(&self.messages),
            parts: Cow::Borrowed(&self.parts),
            calls: Cow::Borrowed(&self.calls),
        }
        .encode()
    }

    pub(in crate::core::session_store::remote::view) fn into_encoded_cache(
        self,
    ) -> Option<EncodedPrefix<'static>> {
        PrefixFields {
            history: Cow::Owned(self.history),
            turns: Cow::Owned(self.turns),
            exchanges: Cow::Owned(self.exchanges),
            messages: Cow::Owned(self.messages),
            parts: Cow::Owned(self.parts),
            calls: Cow::Owned(self.calls),
        }
        .encode()
    }

    pub(in crate::core::session_store::remote::view) fn decode_cache(value: Value) -> Option<Self> {
        let data: Packed<Vec<CallId>> = serde_json::from_value(value).ok()?;
        if data.format != 1 {
            return None;
        }
        let mut left = LIMIT;
        for field in [
            &data.history,
            &data.turns,
            &data.exchanges,
            &data.messages,
            &data.parts,
        ] {
            left = left.checked_sub(field.len())?;
        }
        let mut result = Self::default();
        for call in data.calls {
            left = left.checked_sub(call.len().checked_add(2)?)?;
            result.calls.insert(call).then_some(())?;
        }
        unpack(data.history, |hash| {
            result.history.push(hash);
            Some(())
        })?;
        for (encoded, set) in [
            (data.turns, &mut result.turns),
            (data.exchanges, &mut result.exchanges),
            (data.messages, &mut result.messages),
        ] {
            unpack(encoded, |id| set.insert(Uuid::from_bytes(id)).then_some(()))?;
        }
        unpack(data.parts, |record: [u8; 17]| {
            let id = Uuid::from_slice(&record[..16]).ok()?;
            let explicit = match record[16] {
                0 => false,
                1 => true,
                _ => return None,
            };
            result.parts.insert(id, explicit).is_none().then_some(())
        })?;
        Some(result)
    }
}

#[cfg(test)]
mod tests;
