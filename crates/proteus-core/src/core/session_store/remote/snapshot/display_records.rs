//! Rewindable, snapshot-local render input. No replay-only payloads cross this
//! buffer. Failure to stage the records leaves the original streaming reader.
use super::{JournalKind, JournalRecord};
use crate::{
    core::{HistoryMutationKind, JournalEntry, ModelResponseOutcome},
    domain::{PartId, ThreadId, TurnId},
    model_standard::ContentPart,
};
use anyhow::Result;
use std::{
    collections::HashMap,
    fs::File,
    io::{self, BufRead, BufReader, BufWriter, Seek, Write},
};

const MAX_STAGING_BYTES: u64 = 512 * 1024 * 1024;
// Outliers use the ordinary renderer. The optional staging metadata must not
// grow in proportion to an arbitrarily long journal.
const MAX_STAGED_ROOTS: usize = 4096;

type RootBindings = HashMap<TurnId, ThreadId>;

struct CausalStage {
    roots: RootBindings,
    pending_text: Option<PartId>,
    saw_response: bool,
    plain_history: bool,
}

impl CausalStage {
    fn new() -> Self {
        Self {
            roots: RootBindings::new(),
            pending_text: None,
            saw_response: false,
            plain_history: true,
        }
    }

    fn note(&mut self, record: &JournalRecord) -> bool {
        match &record.entry {
            JournalEntry::HistoryMutated(mutation) => {
                if mutation.mutation == HistoryMutationKind::Replace
                    && mutation.compaction.is_none()
                {
                    return false;
                }
                for part in mutation.messages.iter().flat_map(|message| &message.parts) {
                    if matches!(part.payload, ContentPart::HostedToolActivity { .. }) {
                        return false;
                    }
                    self.plain_history &= matches!(part.payload, ContentPart::Text { .. });
                    if Some(part.part_id) == self.pending_text {
                        self.pending_text = None;
                    }
                }
                !self.saw_response || self.plain_history
            }
            JournalEntry::ModelResponseRecorded(record) => {
                if !self.plain_history || self.pending_text.is_some() {
                    return false;
                }
                let ModelResponseOutcome::Response { response } = &record.outcome else {
                    return false;
                };
                let [message] = response.messages.as_slice() else {
                    return false;
                };
                let [part] = message.parts.as_slice() else {
                    return false;
                };
                if !response.tool_calls.is_empty()
                    || !matches!(part.payload, ContentPart::Text { .. })
                {
                    return false;
                }
                // A positive commit witness for every response proves the normal
                // visibility rule without keeping a global set of committed parts.
                self.saw_response = true;
                self.pending_text = Some(part.part_id);
                true
            }
            JournalEntry::TurnOpened(_) => {
                if let (Some(turn), Some(thread)) = (record.turn_id, record.thread_id) {
                    if self.roots.len() == MAX_STAGED_ROOTS && !self.roots.contains_key(&turn) {
                        return false;
                    }
                    // Match full discovery, which uses the final binding in journal order.
                    self.roots.insert(turn, thread);
                }
                true
            }
            JournalEntry::TurnSettled(_) => true,
            _ => false,
        }
    }

    fn finish(self) -> Option<RootBindings> {
        self.pending_text.is_none().then_some(self.roots)
    }
}

pub(crate) fn is_display_kind(kind: JournalKind) -> bool {
    kind.is_display_record()
}

pub(in crate::core::session_store::remote) struct DisplayRecordWriter {
    writer: Option<BufWriter<File>>,
    remaining: u64,
    count: usize,
    causal_stage: Option<CausalStage>,
}

impl DisplayRecordWriter {
    pub(in crate::core::session_store::remote) fn new() -> Option<Self> {
        Some(Self {
            writer: Some(BufWriter::new(tempfile::tempfile().ok()?)),
            remaining: MAX_STAGING_BYTES,
            count: 0,
            causal_stage: None,
        })
    }

    pub(in crate::core::session_store::remote) fn new_for_replay() -> Option<Self> {
        let mut stage = Self::new()?;
        stage.causal_stage = Some(CausalStage::new());
        Some(stage)
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
            self.causal_stage = None;
        } else {
            self.count += 1;
            if let Some(stage) = &mut self.causal_stage
                && !stage.note(record)
            {
                self.causal_stage = None;
            }
        }
    }

    pub(in crate::core::session_store::remote) fn finish(self) -> Option<DisplayRecords> {
        Some(DisplayRecords {
            file: self.writer?.into_inner().ok()?,
            count: self.count,
            causal_roots: self.causal_stage.and_then(CausalStage::finish),
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
    causal_roots: Option<RootBindings>,
}

impl DisplayRecords {
    #[cfg(test)]
    pub(crate) fn stage_for_test(records: &[JournalRecord]) -> (Self, Option<RootBindings>) {
        let mut stage = DisplayRecordWriter::new_for_replay().unwrap();
        for record in records {
            stage.push(record);
        }
        let mut staged = stage.finish().unwrap();
        let roots = staged.take_causal_roots();
        (staged, roots)
    }

    pub(super) fn take_causal_roots(&mut self) -> Option<RootBindings> {
        self.causal_roots.take()
    }

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
