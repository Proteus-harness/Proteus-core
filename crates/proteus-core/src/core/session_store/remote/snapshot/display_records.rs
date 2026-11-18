//! Rewindable, snapshot-local render input. No replay-only payloads cross this
//! buffer. Failure to stage the records leaves the original streaming reader.
use super::{JournalKind, JournalRecord};
use anyhow::Result;
use std::{
    fs::File,
    io::{self, BufRead, BufReader, BufWriter, Seek, Write},
};

const MAX_STAGING_BYTES: u64 = 512 * 1024 * 1024;

pub(crate) fn is_display_kind(kind: JournalKind) -> bool {
    matches!(
        kind,
        JournalKind::HistoryMutated
            | JournalKind::ModelMessageRecorded
            | JournalKind::ModelResponseRecorded
            | JournalKind::ToolCallRecorded
            | JournalKind::ToolResultRecorded
            | JournalKind::ToolEffectRecorded
            | JournalKind::SubagentSessionBoundRecorded
            | JournalKind::TurnOpened
            | JournalKind::TurnSettled
    )
}

pub(in crate::core::session_store::remote) struct DisplayRecordWriter {
    writer: Option<BufWriter<File>>,
    remaining: u64,
    count: usize,
}

impl DisplayRecordWriter {
    pub(in crate::core::session_store::remote) fn new() -> Option<Self> {
        Some(Self {
            writer: Some(BufWriter::new(tempfile::tempfile().ok()?)),
            remaining: MAX_STAGING_BYTES,
            count: 0,
        })
    }

    pub(in crate::core::session_store::remote) fn push(&mut self, record: &JournalRecord) {
        if self.writer.is_none() {
            return;
        }
        let result = serde_json::to_writer(&mut *self, record)
            .map_err(io::Error::other)
            .and_then(|()| self.write_all(b"\n"));
        if result.is_err() {
            self.writer = None;
        } else {
            self.count += 1;
        }
    }

    pub(in crate::core::session_store::remote) fn finish(self) -> Option<DisplayRecords> {
        Some(DisplayRecords {
            file: self.writer?.into_inner().ok()?,
            count: self.count,
        })
    }
}

impl Write for DisplayRecordWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(io::Error::other("transcript staging exceeds byte budget"));
        }
        let written = self.writer.as_mut().expect("active stage").write(bytes)?;
        self.remaining -= written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.as_mut().expect("active stage").flush()
    }
}

pub(crate) struct DisplayRecords {
    file: File,
    count: usize,
}

impl DisplayRecords {
    pub(crate) fn scan(
        &mut self,
        mut consume: impl FnMut(JournalRecord) -> Result<()>,
    ) -> Result<()> {
        self.file.rewind()?;
        let mut reader = BufReader::new(&mut self.file);
        let mut count = 0;
        let mut line = Vec::new();
        while reader.read_until(b'\n', &mut line)? > 0 {
            anyhow::ensure!(
                line.ends_with(b"\n"),
                "incomplete transcript staging record"
            );
            consume(serde_json::from_slice(&line)?)?;
            count += 1;
            line.clear();
        }
        anyhow::ensure!(
            count == self.count,
            "transcript staging record count changed"
        );
        Ok(())
    }
}
