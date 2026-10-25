//! Disposable, source-validated read models. Never an audit/replay authority.
use std::{
    fs::{self, Metadata, OpenOptions},
    io::{self, ErrorKind, Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{core::session_journal::JOURNAL_SCHEMA_VERSION, domain::new_record_id};

const SCHEMA_VERSION: u32 = 1;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_BLOBS: usize = 8192;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Identity {
    device: u64,
    inode: u64,
    mode: u32,
}
impl From<&Metadata> for Identity {
    fn from(m: &Metadata) -> Self {
        Self {
            device: m.dev(),
            inode: m.ino(),
            mode: m.mode(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct FileStamp {
    identity: Identity,
    bytes: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl From<Metadata> for FileStamp {
    fn from(m: Metadata) -> Self {
        Self {
            identity: (&m).into(),
            bytes: m.len(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
        }
    }
}

/// Strong local source identity; unrelated files in the session do not invalidate it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceFingerprint {
    directory: Identity,
    metadata: FileStamp,
    journal: Option<FileStamp>,
    blobs_directory: Option<FileStamp>,
    blobs: Vec<(Vec<u8>, FileStamp)>,
}
impl SourceFingerprint {
    pub(crate) fn read(dir: &Path, max_blobs: usize) -> Option<Self> {
        let directory = fs::symlink_metadata(dir).ok()?;
        if !directory.is_dir() {
            return None;
        }
        let metadata = required_stamp(&dir.join("session.json"), false)?;
        let journal = optional_stamp(&dir.join("journal.jsonl"), false)?;
        let blob_dir = dir.join("blobs");
        let blobs_directory = optional_stamp(&blob_dir, true)?;
        let mut blobs = Vec::new();
        if blobs_directory.is_some() {
            for entry in fs::read_dir(&blob_dir).ok()? {
                let entry = entry.ok()?;
                if blobs.len() >= max_blobs {
                    return None;
                }
                let stamp = required_stamp(&entry.path(), false)?;
                blobs.push((entry.file_name().as_bytes().to_vec(), stamp));
            }
            blobs.sort_by(|a, b| a.0.cmp(&b.0));
            // Detect additions/removals while enumerating the directory.
            if optional_stamp(&blob_dir, true)? != blobs_directory {
                return None;
            }
        }
        Some(Self {
            directory: (&directory).into(),
            metadata,
            journal,
            blobs_directory,
            blobs,
        })
    }

    pub(crate) fn file_stamps(&self) -> usize {
        self.blobs.len() + 4
    }
}

fn required_stamp(path: &Path, directory: bool) -> Option<FileStamp> {
    let metadata = fs::symlink_metadata(path).ok()?;
    (if directory {
        metadata.is_dir()
    } else {
        metadata.is_file()
    })
    .then(|| metadata.into())
}
fn optional_stamp(path: &Path, directory: bool) -> Option<Option<FileStamp>> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if if directory {
                metadata.is_dir()
            } else {
                metadata.is_file()
            } =>
        {
            Some(Some(metadata.into()))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Some(None),
        _ => None,
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    schema_version: u32,
    journal_schema_version: u32,
    source: SourceFingerprint,
    checksum: String,
    data: T,
}

fn cache_path(dir: &Path, name: &str) -> Option<PathBuf> {
    (!name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .then(|| dir.join(format!(".read-{name}.json")))
}

pub(crate) fn load<T: Serialize + DeserializeOwned>(
    dir: &Path,
    name: &str,
) -> Option<(SourceFingerprint, T)> {
    let (source, data, fresh) = load_any(dir, name)?;
    fresh.then_some((source, data))
}

/// A stale model is display-only until its caller refreshes it from the journal.
/// The returned fingerprint always belongs to the cached model, not current files.
pub(crate) fn load_any<T: Serialize + DeserializeOwned>(
    dir: &Path,
    name: &str,
) -> Option<(SourceFingerprint, T, bool)> {
    let before = SourceFingerprint::read(dir, MAX_BLOBS)?;
    let path = cache_path(dir, name)?;
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES as u64 {
        return None;
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    fs::File::open(path)
        .ok()?
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_BYTES {
        return None;
    }
    let envelope: Envelope<T> = serde_json::from_slice(&bytes).ok()?;
    if envelope.schema_version != SCHEMA_VERSION
        || envelope.journal_schema_version != JOURNAL_SCHEMA_VERSION
        || checksum(&envelope.data)? != envelope.checksum
    {
        return None;
    }
    let after = SourceFingerprint::read(dir, MAX_BLOBS)?;
    let fresh = envelope.source == before && after == before;
    Some((envelope.source, envelope.data, fresh))
}

pub(crate) fn save<T: Serialize>(
    dir: &Path,
    name: &str,
    source: &SourceFingerprint,
    data: &T,
) -> bool {
    save_inner(dir, name, source, data).is_some()
}
fn save_inner<T: Serialize>(
    dir: &Path,
    name: &str,
    source: &SourceFingerprint,
    data: &T,
) -> Option<()> {
    let path = cache_path(dir, name)?;
    if SourceFingerprint::read(dir, MAX_BLOBS).as_ref() != Some(source) {
        return None;
    }
    let envelope = Envelope {
        schema_version: SCHEMA_VERSION,
        journal_schema_version: JOURNAL_SCHEMA_VERSION,
        source: source.clone(),
        checksum: checksum(data)?,
        data,
    };
    let bytes = serialize_bounded(&envelope)?;
    let temp = dir.join(format!(".read-{name}.tmp.{}", new_record_id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temp)
            .ok()?;
        file.write_all(&bytes).ok()?;
        file.sync_data().ok()?;
        if SourceFingerprint::read(dir, MAX_BLOBS).as_ref() != Some(source) {
            return None;
        }
        fs::rename(&temp, &path).ok()?;
        Some(())
    })();
    let _ = fs::remove_file(temp);
    result
}

fn checksum<T: Serialize>(data: &T) -> Option<String> {
    Some(
        digest(&SHA256, &serialize_bounded(data)?)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}
fn serialize_bounded<T: Serialize>(data: &T) -> Option<Vec<u8>> {
    struct Bounded(Vec<u8>);
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > MAX_BYTES.saturating_sub(self.0.len()) {
                return Err(io::Error::other("read cache exceeds size limit"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Bounded(Vec::new());
    serde_json::to_writer(&mut writer, data).ok()?;
    Some(writer.0)
}

#[cfg(test)]
mod tests;
