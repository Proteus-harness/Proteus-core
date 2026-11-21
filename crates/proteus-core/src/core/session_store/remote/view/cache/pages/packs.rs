//! Several sealed logical leaves can share one content-addressed storage value.
//! Singletons (especially every leaf touching the latest UI page) retain their
//! original key and payload. No duplicate copy of a packed leaf is required.
use super::super::{hash, read_value, write_content_value};
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use miniz_oxide::{deflate::compress_to_vec, inflate::decompress_to_vec_with_limit};
use serde_json::{Map, Value};

// Compact JSON of {"deflate_v1":[]} plus quoted base64 strings and commas.
const FRAME_OVERHEAD: usize = br#"{"deflate_v1":[]}"#.len();

const MAX_LEAVES: usize = 8;
const MAX_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PackRef {
    pub(super) first: usize,
    pub(super) count: usize,
    pub(super) hash: String,
}

#[derive(Clone, Copy)]
pub(super) enum Encoding {
    Json,
    DeflateV1,
}

impl Encoding {
    /// Only inflate overlapping leaves. The manifest's declared logical size is
    /// independently constrained by the physical pack cap before allocation.
    pub(super) fn decode(self, body: Value, bytes: usize) -> Option<Value> {
        match self {
            Self::Json => Some(body),
            Self::DeflateV1 => {
                let data = STANDARD.decode(body.as_str()?).ok()?;
                let raw = decompress_to_vec_with_limit(&data, bytes.min(MAX_BYTES)).ok()?;
                if raw.len() != bytes {
                    return None;
                }
                serde_json::from_slice(&raw).ok()
            }
        }
    }
}

impl PackRef {
    pub(super) fn read(
        &self,
        store: &RemoteSessionStore,
        pages: &[PageRef],
    ) -> Option<(Encoding, Vec<Value>)> {
        let value = read_value(store, &format!("display-data-{}", self.hash))?;
        // A disposable header's byte counts are not authority for a returned
        // object's size. Reject an oversized body before hashing its contents.
        let size = encoded_len(&value, MAX_BYTES)?;
        if hash(&value).as_deref() != Some(&self.hash) {
            return None;
        }
        match value {
            Value::Object(mut object) if object.len() == 1 => {
                let Value::Array(leaves) = object.remove("deflate_v1")? else {
                    return None;
                };
                let smallest = pages.iter().map(|page| page.bytes).min()?;
                (size <= 2 * smallest
                    && leaves.len() == self.count
                    && leaves.iter().all(Value::is_string))
                .then_some((Encoding::DeflateV1, leaves))
            }
            _ => None,
        }
    }
}

pub(super) fn valid_spans(section: &Section) -> bool {
    if section.packs.is_empty() {
        return true;
    }
    // Never cover the latest PAGE_ROWS, even if they span many byte-split leaves.
    let mut tail = section.pages.len();
    let mut tail_rows = 0;
    while tail > 0 && tail_rows < PAGE_ROWS {
        tail -= 1;
        tail_rows += section.pages[tail].rows;
    }
    let mut after = 0;
    section.packs.iter().all(|pack| {
        if !(2..=MAX_LEAVES).contains(&pack.count)
            || pack.first < after
            || pack.hash.len() != 64
            || !pack.hash.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return false;
        }
        let Some(end) = pack
            .first
            .checked_add(pack.count)
            .filter(|&end| end <= tail)
        else {
            return false;
        };
        after = end;
        section.pages[pack.first..end]
            .iter()
            .try_fold(1usize, |n, p| n.checked_add(1)?.checked_add(p.bytes))
            .is_some_and(|n| n <= MAX_BYTES)
    })
}

#[derive(Default)]
pub(super) struct PendingPack {
    first: usize,
    // Only the first logical body might be published as a singleton.
    first_body: Option<Value>,
    frames: Vec<String>,
    logical_bytes: usize,
    wire_bytes: usize,
    smallest: usize,
}

impl PendingPack {
    pub(super) fn push(
        &mut self,
        store: &RemoteSessionStore,
        source: &RemoteCacheSource,
        section: &mut Section,
        value: Value,
        historical: bool,
    ) -> Option<()> {
        let index = section.pages.len() - 1;
        let bytes = section.pages[index].bytes;
        if !historical || bytes + 2 > MAX_BYTES {
            self.flush(store, source, section)?;
            return write_content_value(store, source, &section.pages[index].hash, value);
        }
        // write_pages already counted this local leaf, and the guard above caps the reservation.
        let mut raw = Vec::with_capacity(bytes);
        serde_json::to_writer(&mut raw, &value).ok()?;
        // A custom Serialize impl need not emit identical bytes on both paths.
        // Keep the original representation when the manifest's byte count differs.
        if raw.len() != bytes {
            self.flush(store, source, section)?;
            return write_content_value(store, source, &section.pages[index].hash, value);
        }
        let frame = STANDARD.encode(compress_to_vec(&raw, 1));
        let cost = frame.len() + 2;
        let wire = self.wire_bytes + cost + 1;
        // Keep a full-pack read within twice the smallest included leaf. Poorly
        // compressible or mixed-size pages naturally form shorter packs.
        if !self.frames.is_empty()
            && (self.frames.len() == MAX_LEAVES
                || self.logical_bytes + bytes + 1 > MAX_BYTES
                || wire > MAX_BYTES
                || wire > 2 * self.smallest.min(bytes))
        {
            self.flush(store, source, section)?;
        }
        if self.frames.is_empty() {
            self.first = index;
            self.smallest = bytes;
            self.logical_bytes = 2 + bytes;
            self.wire_bytes = FRAME_OVERHEAD + cost;
            self.first_body = Some(value);
        } else {
            self.first_body = None;
            self.smallest = self.smallest.min(bytes);
            self.logical_bytes += 1 + bytes;
            self.wire_bytes += 1 + cost;
        }
        self.frames.push(frame);
        Some(())
    }

    pub(super) fn flush(
        &mut self,
        store: &RemoteSessionStore,
        source: &RemoteCacheSource,
        section: &mut Section,
    ) -> Option<()> {
        let frames = std::mem::take(&mut self.frames);
        let count = frames.len();
        match count {
            0 => Some(()),
            1 => write_content_value(
                store,
                source,
                &section.pages[self.first].hash,
                self.first_body.take()?,
            ),
            _ => {
                let leaves = Value::Array(frames.into_iter().map(Value::String).collect());
                let value = Value::Object(Map::from_iter([("deflate_v1".into(), leaves)]));
                let digest = hash(&value)?;
                write_content_value(store, source, &digest, value)?;
                section.packs.push(PackRef {
                    first: self.first,
                    count,
                    hash: digest,
                });
                Some(())
            }
        }
    }
}
