//! The append certificate stores exact keys, not lossy fingerprints of IDs.
//! Its outer cache envelope still validates the complete JSON value by SHA.
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use proteus_contracts::contracts::SESSION_STORAGE_MAX_CACHE_BYTES as LIMIT;
use serde_json::Value;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::session_store::remote::view) struct Packed<C> {
    format: u8,
    history: String,
    turns: String,
    exchanges: String,
    messages: String,
    // Each 17-byte record is UUID plus a strict 0/1 explicit membership flag.
    parts: String,
    calls: C,
}

impl<C: Serialize> Packed<C> {
    pub(in crate::core::session_store::remote::view) fn into_bounded_value(self) -> Option<Value> {
        crate::core::session_store::json_size::encoded_len(&self, LIMIT)?;
        // Match the derived field order, including when Map preserves insertion order.
        let mut object = serde_json::Map::with_capacity(7);
        object.insert("format".to_owned(), Value::from(self.format));
        object.insert("history".to_owned(), Value::String(self.history));
        object.insert("turns".to_owned(), Value::String(self.turns));
        object.insert("exchanges".to_owned(), Value::String(self.exchanges));
        object.insert("messages".to_owned(), Value::String(self.messages));
        object.insert("parts".to_owned(), Value::String(self.parts));
        object.insert("calls".to_owned(), serde_json::to_value(self.calls).ok()?);
        Some(Value::Object(object))
    }
}

fn pack<const N: usize>(records: impl Iterator<Item = [u8; N]>, n: usize) -> String {
    const BATCH_RECORDS: usize = 48;
    if n <= BATCH_RECORDS {
        let mut bytes = Vec::with_capacity(n * N);
        for item in records {
            bytes.extend_from_slice(&item);
        }
        return STANDARD.encode(bytes);
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
    String::from_utf8(output).expect("Base64 is UTF-8")
}

fn ids(set: &HashSet<Uuid>) -> String {
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

impl PrefixIdentities {
    pub(in crate::core::session_store::remote::view) fn encode_cache(
        &self,
    ) -> Option<Packed<&HashSet<CallId>>> {
        // Bound all binary allocations before creating them. The caller also
        // checks the actual serialized JSON byte count (including escapes).
        let mut left = LIMIT;
        for (count, width) in [
            (self.history.len(), 32),
            (self.turns.len(), 16),
            (self.exchanges.len(), 16),
            (self.messages.len(), 16),
            (self.parts.len(), 17),
        ] {
            left = left.checked_sub(base64::encoded_len(count.checked_mul(width)?, true)?)?;
        }
        for call in &self.calls {
            left = left.checked_sub(call.len().checked_add(2)?)?;
        }
        if !self.explicit_committed_parts.is_subset(&self.parts) {
            return None;
        }
        let parts = pack(
            self.parts.iter().map(|id| {
                let mut record = [0; 17];
                record[..16].copy_from_slice(id.as_bytes());
                record[16] = u8::from(self.explicit_committed_parts.contains(id));
                record
            }),
            self.parts.len(),
        );
        Some(Packed {
            format: 1,
            history: pack(self.history.iter().copied(), self.history.len()),
            turns: ids(&self.turns),
            exchanges: ids(&self.exchanges),
            messages: ids(&self.messages),
            parts,
            calls: &self.calls,
        })
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
            result.parts.insert(id).then_some(())?;
            match record[16] {
                0 => Some(()),
                1 => result.explicit_committed_parts.insert(id).then_some(()),
                _ => None,
            }
        })?;
        Some(result)
    }
}

#[cfg(test)]
mod tests;
