//! Decode one complete JSONL record at a time; never retain the whole file buffer.
use super::*;
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
};

pub(crate) fn load_records(
    session_dir: &Path,
    session_id: SessionId,
) -> Result<Vec<JournalRecord>> {
    #[cfg(feature = "memory-diagnostics")]
    let memory = crate::memory_diagnostics::Scope::new("journal.load_records");
    let mut records = Vec::new();
    scan_records(session_dir, session_id, |record| {
        records.push(record);
        Ok(())
    })?;
    #[cfg(feature = "memory-diagnostics")]
    memory.sample("journal.raw_records_loaded", &[("records", records.len())]);
    Ok(records)
}

/// A fixed complete prefix on one open journal inode. Repeated serial passes
/// cannot observe later appends or a replacement journal at the same path.
#[derive(Debug)]
pub(crate) struct JournalReadSnapshot {
    file: Option<File>,
    path: PathBuf,
    session_dir: PathBuf,
    session_id: SessionId,
    complete_len: u64,
    scanned_prefix: Option<ring::digest::Digest>,
}

impl JournalReadSnapshot {
    pub(crate) fn open(session_dir: &Path, session_id: SessionId) -> Result<Self> {
        let path = journal_path(session_dir);
        let mut file = match File::open(&path) {
            Ok(file) => Some(file),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {}", path.display()));
            }
        };
        let complete_len = if let Some(file) = &mut file {
            let metadata = file.metadata()?;
            if !metadata.is_file() {
                bail!("journal {} is not a regular file", path.display());
            }
            complete_jsonl_offset(file, metadata.len())?
        } else {
            0
        };
        Ok(Self {
            file,
            path,
            session_dir: session_dir.to_owned(),
            session_id,
            complete_len,
            scanned_prefix: None,
        })
    }

    pub(crate) fn scan(&mut self, consume: impl FnMut(JournalRecord) -> Result<()>) -> Result<()> {
        self.scan_inner(|_| true, consume)
    }

    /// Derived passes may omit payload hydration only after a full typed scan.
    /// Every pass still verifies the original raw prefix before it succeeds.
    pub(crate) fn scan_selected(
        &mut self,
        select: fn(JournalKind) -> bool,
        consume: impl FnMut(JournalRecord) -> Result<()>,
    ) -> Result<()> {
        anyhow::ensure!(
            self.scanned_prefix.is_some(),
            "selected journal scan requires a complete typed scan"
        );
        self.scan_inner(select, consume)
    }

    fn verify_prefix(&mut self, actual: ring::digest::Digest) -> Result<()> {
        if let Some(expected) = &self.scanned_prefix {
            anyhow::ensure!(
                expected.as_ref() == actual.as_ref(),
                "journal {} complete prefix changed between snapshot passes",
                self.path.display()
            );
        } else {
            self.scanned_prefix = Some(actual);
        }
        Ok(())
    }

    fn scan_inner(
        &mut self,
        select: fn(JournalKind) -> bool,
        mut consume: impl FnMut(JournalRecord) -> Result<()>,
    ) -> Result<()> {
        let mut prefix = ring::digest::Context::new(&SHA256);
        let Some(file) = self.file.as_mut() else {
            return self.verify_prefix(prefix.finish());
        };
        file.seek(SeekFrom::Start(0))?;
        let path = &self.path;
        let session_dir = &self.session_dir;
        let expected_session_id = self.session_id;
        let complete_len = self.complete_len;
        let mut reader = BufReader::with_capacity(64 * 1024, file.take(complete_len));
        let mut line = Vec::new();
        let mut index = 0;
        loop {
            line.clear();
            if reader
                .read_until(b'\n', &mut line)
                .with_context(|| format!("failed to read {}", path.display()))?
                == 0
            {
                anyhow::ensure!(
                    reader.get_ref().limit() == 0,
                    "journal {} shortened during snapshot read",
                    path.display()
                );
                drop(reader);
                return self.verify_prefix(prefix.finish());
            }
            prefix.update(&line);
            index += 1;
            if !line.ends_with(b"\n") {
                bail!("journal {} lost a complete snapshot line", path.display());
            }
            line.pop();
            if line.is_empty() {
                bail!(
                    "journal {} contains an empty line at {}",
                    path.display(),
                    index
                );
            }
            let stored: StoredJournalRecord = serde_json::from_slice(&line).with_context(|| {
                format!("failed to parse journal {} line {}", path.display(), index)
            })?;
            if stored.schema_version != JOURNAL_SCHEMA_VERSION {
                bail!(
                    "unsupported journal schema_version {} in {} line {}; expected {}",
                    stored.schema_version,
                    path.display(),
                    index,
                    JOURNAL_SCHEMA_VERSION
                );
            }
            if stored.session_id != expected_session_id {
                bail!(
                    "journal {} line {} belongs to session {}, expected {}",
                    path.display(),
                    index,
                    stored.session_id,
                    expected_session_id
                );
            }
            if !select(stored.kind) {
                continue;
            }
            let entry =
                hydrate_entry(session_dir, stored.kind, stored.payload).with_context(|| {
                    format!(
                        "invalid journal payload in {} line {}",
                        path.display(),
                        index
                    )
                })?;
            consume(JournalRecord {
                schema_version: stored.schema_version,
                record_id: stored.record_id,
                session_seq: stored.session_seq,
                timestamp_ms: stored.timestamp_ms,
                session_id: stored.session_id,
                execution_id: stored.execution_id,
                thread_id: stored.thread_id,
                turn_id: stored.turn_id,
                entry,
            })?;
        }
    }
}

pub(super) fn scan_records(
    session_dir: &Path,
    expected_session_id: SessionId,
    consume: impl FnMut(JournalRecord) -> Result<()>,
) -> Result<()> {
    JournalReadSnapshot::open(session_dir, expected_session_id)?.scan(consume)
}

/// Last complete JSONL boundary. A clean file needs only one small tail read.
pub(super) fn complete_jsonl_offset(file: &mut File, length: u64) -> Result<u64> {
    let mut end = length;
    let mut buffer = [0_u8; 8192];
    while end > 0 {
        let start = end.saturating_sub(buffer.len() as u64);
        let count = (end - start) as usize;
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut buffer[..count])?;
        if let Some(index) = buffer[..count].iter().rposition(|byte| *byte == b'\n') {
            return Ok(start + index as u64 + 1);
        }
        end = start;
    }
    Ok(0)
}
