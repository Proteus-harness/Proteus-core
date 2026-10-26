use std::{
    collections::{BTreeSet, VecDeque},
    fs::Metadata,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use super::Telemetry;
use crate::domain::SessionId;

const MAX_LOGS: usize = 16;
const MAX_ENTRIES: usize = 128;
// Serialized telemetry budget; envelopes, tool outputs and compaction text are never retained.
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRY_BYTES: usize = 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Key {
    path: PathBuf,
    session_id: Option<SessionId>,
    session_dir: Option<PathBuf>,
}

impl Key {
    pub(super) fn new(
        path: &Path,
        session_id: Option<SessionId>,
        session_dir: Option<&Path>,
    ) -> Self {
        Self {
            path: path.to_path_buf(),
            session_id,
            session_dir: session_dir.map(Path::to_path_buf),
        }
    }
}

#[derive(PartialEq, Eq)]
pub(super) struct SourceStamp {
    identity: [u64; 7],
}

impl SourceStamp {
    pub(super) fn read(path: &Path) -> Option<Self> {
        Self::from_metadata(std::fs::metadata(path).ok()?)
    }

    pub(super) fn from_metadata(metadata: Metadata) -> Option<Self> {
        if !metadata.is_file() {
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Some(Self {
                identity: [
                    metadata.dev(),
                    metadata.ino(),
                    metadata.len(),
                    metadata.mtime() as u64,
                    metadata.mtime_nsec() as u64,
                    metadata.ctime() as u64,
                    metadata.ctime_nsec() as u64,
                ],
            })
        }
        // Without a strong change stamp, remain uncached rather than miss same-size rewrites.
        #[cfg(not(unix))]
        {
            None
        }
    }
}

struct Entry {
    key: Key,
    source: SourceStamp,
    telemetry: Arc<Telemetry>,
    bytes: usize,
}

#[derive(Default)]
struct Cache {
    entries: VecDeque<Entry>,
    bytes: usize,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

pub(super) fn get(key: &Key, source: &SourceStamp) -> Option<Arc<Telemetry>> {
    let mut cache = cache().lock().unwrap_or_else(|poison| poison.into_inner());
    let index = cache.entries.iter().position(|entry| entry.key == *key)?;
    let entry = cache.entries.remove(index).unwrap();
    if entry.source != *source {
        cache.bytes -= entry.bytes;
        return None;
    }
    let telemetry = entry.telemetry.clone();
    cache.entries.push_back(entry);
    Some(telemetry)
}

pub(super) fn insert(key: Key, source: SourceStamp, telemetry: Arc<Telemetry>) {
    // Counting writer bounds work without allocating another serialized copy of the DTO.
    let mut count = Budget(0);
    if serde_json::to_writer(&mut count, telemetry.as_ref()).is_err() {
        return;
    }
    let mut cache = cache().lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(index) = cache.entries.iter().position(|entry| entry.key == key) {
        let old = cache.entries.remove(index).unwrap();
        cache.bytes -= old.bytes;
    }
    cache.bytes += count.0;
    cache.entries.push_back(Entry {
        key,
        source,
        telemetry,
        bytes: count.0,
    });
    while cache.entries.len() > MAX_ENTRIES
        || cache.bytes > MAX_BYTES
        || cache
            .entries
            .iter()
            .map(|entry| &entry.key.path)
            .collect::<BTreeSet<_>>()
            .len()
            > MAX_LOGS
    {
        let old = cache.entries.pop_front().unwrap();
        cache.bytes -= old.bytes;
    }
}

struct Budget(usize);
impl Write for Budget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_ENTRY_BYTES.saturating_sub(self.0) {
            return Err(io::Error::other("telemetry cache entry exceeds budget"));
        }
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
