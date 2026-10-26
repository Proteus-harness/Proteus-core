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
    let mut records = Vec::new();
    scan_records(session_dir, session_id, |record| {
        records.push(record);
        Ok(())
    })?;
    Ok(records)
}

pub(super) fn scan_records(
    session_dir: &Path,
    expected_session_id: SessionId,
    mut consume: impl FnMut(JournalRecord) -> Result<()>,
) -> Result<()> {
    let path = journal_path(session_dir);
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    // Bound this read to a complete prefix, including when a writer is appending.
    // An arbitrarily long torn tail is scanned backwards without allocating it.
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("journal {} is not a regular file", path.display());
    }
    let length = metadata.len();
    let complete_len = complete_jsonl_offset(&mut file, length)?;
    file.seek(SeekFrom::Start(0))?;
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
            return Ok(());
        }
        index += 1;
        if !line.ends_with(b"\n") {
            return Ok(());
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
        let entry = hydrate_entry(session_dir, stored.kind, stored.payload).with_context(|| {
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
