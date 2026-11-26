//! Rewindable, snapshot-local render input. No replay-only payloads cross this
//! buffer. Failure to stage the records leaves the original streaming reader.
use super::super::view::PrefixIdentities;
use super::{JournalKind, JournalRecord};
use crate::{
    core::{HistoryMutated, HistoryMutationKind, JournalEntry, ModelResponseOutcome},
    domain::{PartId, ThreadId, TurnId},
    model_standard::ContentPart,
};
use anyhow::Result;
use std::{
    collections::HashMap,
    fs::File,
    io::{self, BufRead, BufReader, BufWriter, Read, Seek, Write},
};

mod text_append;
use text_append::{MAX_ROWS, MIN_ROWS};
pub(crate) use text_append::{StagedTextAppend, StagedTextRow};

pub(crate) enum StagedRenderRecord {
    Journal(JournalRecord),
    Text(StagedTextAppend),
}

const MAX_STAGING_BYTES: u64 = 512 * 1024 * 1024;
const MAX_RETAINED_FINAL_SCAN_BYTES: usize = 4 * 1024 * 1024;
// Only seed typical large projected/strict pages. Higher outliers retain
// incremental growth and the existing staging limit.
const MAX_SINGLE_RECORD_RESERVE: u64 = 64 * 1024 * 1024;
const PREFIX_WITNESS_CHUNK_BYTES: usize = 8 * 1024;

fn single_record_reserve(count: usize, written: u64) -> usize {
    if count == 1
        && written > MAX_RETAINED_FINAL_SCAN_BYTES as u64
        && written <= MAX_SINGLE_RECORD_RESERVE
    {
        written as usize
    } else {
        0
    }
}
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

    fn note<const CHECK_COMPACT: bool>(&mut self, record: &JournalRecord) -> Option<bool> {
        match &record.entry {
            JournalEntry::HistoryMutated(mutation) => {
                if mutation.mutation == HistoryMutationKind::Replace
                    && mutation.compaction.is_none()
                {
                    return None;
                }
                let mut compact = CHECK_COMPACT && staged_text_shape(mutation);
                // On the writer-proven first large line, check metadata and
                // singleton shape during the causal walk already required.
                // The stock plain-history bit establishes that every part is text.
                for message in &mutation.messages {
                    if compact {
                        compact = message.parts.len() == 1
                            && message.metadata.is_null()
                            && message.name.is_none()
                            && message.tool_call_id.is_none();
                    }
                    for part in &message.parts {
                        if matches!(part.payload, ContentPart::HostedToolActivity { .. }) {
                            return None;
                        }
                        let text = matches!(part.payload, ContentPart::Text { .. });
                        self.plain_history &= text;
                        compact &= text;
                        if Some(part.part_id) == self.pending_text {
                            self.pending_text = None;
                        }
                    }
                }
                (!self.saw_response || self.plain_history).then_some(compact && self.plain_history)
            }
            JournalEntry::ModelResponseRecorded(record) => {
                if !self.plain_history || self.pending_text.is_some() {
                    return None;
                }
                let ModelResponseOutcome::Response { response } = &record.outcome else {
                    return None;
                };
                let [message] = response.messages.as_slice() else {
                    return None;
                };
                let [part] = message.parts.as_slice() else {
                    return None;
                };
                if !response.tool_calls.is_empty()
                    || !matches!(part.payload, ContentPart::Text { .. })
                {
                    return None;
                }
                // A positive commit witness for every response proves the normal
                // visibility rule without keeping a global set of committed parts.
                self.saw_response = true;
                self.pending_text = Some(part.part_id);
                Some(false)
            }
            JournalEntry::TurnOpened(_) => {
                if let (Some(turn), Some(thread)) = (record.turn_id, record.thread_id) {
                    if self.roots.len() == MAX_STAGED_ROOTS && !self.roots.contains_key(&turn) {
                        return None;
                    }
                    // Match full discovery, which uses the final binding in journal order.
                    self.roots.insert(turn, thread);
                }
                Some(false)
            }
            JournalEntry::TurnSettled(_) => Some(false),
            _ => None,
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
    compact_text: bool,
}

impl DisplayRecordWriter {
    pub(in crate::core::session_store::remote) fn new() -> Option<Self> {
        Some(Self {
            writer: Some(BufWriter::new(tempfile::tempfile().ok()?)),
            remaining: MAX_STAGING_BYTES,
            count: 0,
            causal_stage: None,
            compact_text: false,
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
            if let Some(stage) = &mut self.causal_stage {
                // Never inspect compact-only properties for ordinary small
                // or multi-record stages. This flag only permits trying the
                // actual first line in the narrower reader.
                let hint =
                    if single_record_reserve(self.count, MAX_STAGING_BYTES - self.remaining) != 0 {
                        stage.note::<true>(record)
                    } else {
                        stage.note::<false>(record)
                    };
                self.compact_text = hint.unwrap_or(false);
                if hint.is_none() {
                    self.causal_stage = None;
                }
            }
        }
    }

    pub(in crate::core::session_store::remote) fn finish(self) -> Option<DisplayRecords> {
        Some(DisplayRecords {
            file: self.writer?.into_inner().ok()?,
            count: self.count,
            // The bounded writer counts actual accepted bytes, including LF.
            // A failed write or flush does not produce a stage.
            single_record_reserve: single_record_reserve(
                self.count,
                MAX_STAGING_BYTES - self.remaining,
            ),
            causal_roots: self.causal_stage.and_then(CausalStage::finish),
            prefix_witness: None,
            compact_text: self.compact_text,
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
    single_record_reserve: usize,
    causal_roots: Option<RootBindings>,
    prefix_witness: Option<PrefixWitness>,
    compact_text: bool,
}

// Private to this staged file, and consumed by its next prefix scan.
// A successfully rendered record isn't a certificate for a later read.
struct PrefixWitness {
    sha256: ring::digest::Digest,
    prefix: PrefixIdentities,
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

    pub(super) fn is_single_record(&self) -> bool {
        self.count == 1
    }

    pub(super) fn has_compact_text_hint(&self) -> bool {
        self.count == 1 && self.compact_text && self.causal_roots.is_some()
    }

    pub(super) fn take_causal_roots(&mut self) -> Option<RootBindings> {
        self.causal_roots.take()
    }

    pub(crate) fn scan(
        &mut self,
        mut consume: impl FnMut(JournalRecord) -> Result<()>,
    ) -> Result<()> {
        self.scan_lines(|line, last, _| {
            let record = serde_json::from_slice(line)?;
            release_scan_line(line, last);
            consume(record)
        })
    }

    pub(super) fn scan_capturing_prefix(
        &mut self,
        mut consume: impl FnMut(StagedRenderRecord) -> Result<()>,
    ) -> Result<()> {
        self.prefix_witness = None;
        let expected_bytes = self.single_record_reserve;
        if self.count != 1 || expected_bytes == 0 {
            return self.scan(|record| consume(StagedRenderRecord::Journal(record)));
        }

        let compact_text = self.compact_text;
        let mut witness = None;
        self.scan_lines(|line, last, index| {
            if compact_text
                && index == 0
                && line.len() == expected_bytes
                && let Some(append) = StagedTextAppend::decode(line)
            {
                let sha256 = ring::digest::digest(&ring::digest::SHA256, line);
                release_scan_line(line, last);
                let prefix =
                    PrefixIdentities::from_staged_text(append.turn_id, append.identities());
                consume(StagedRenderRecord::Text(append))?;
                witness = Some(PrefixWitness { sha256, prefix });
                return Ok(());
            }
            // The original complete slice is still intact. Propagate only the
            // original decoder's result/error on any miss.
            let record: JournalRecord = serde_json::from_slice(line)?;
            let sha256 = (index == 0 && line.len() == expected_bytes && prefix_candidate(&record))
                .then(|| ring::digest::digest(&ring::digest::SHA256, line));
            release_scan_line(line, last);
            if let Some(sha256) = sha256 {
                let mut prefix = PrefixIdentities::default();
                prefix.visit(&record, false);
                witness = Some(PrefixWitness { sha256, prefix });
            }
            consume(StagedRenderRecord::Journal(record))
        })?;
        // Only commit after the renderer and physical EOF/count checks succeed.
        self.prefix_witness = witness;
        Ok(())
    }

    pub(super) fn scan_prefix(&mut self) -> Result<PrefixIdentities> {
        let mut prefix = PrefixIdentities::default();
        let Some(witness) = self.prefix_witness.take() else {
            self.scan(|record| {
                prefix.visit_owned_prefix(record);
                Ok(())
            })?;
            return Ok(prefix);
        };
        let expected_bytes = self.single_record_reserve;
        if self.count == 1
            && expected_bytes != 0
            && self
                .file
                .rewind()
                .and_then(|()| {
                    matches_prefix_line(&mut self.file, expected_bytes, &witness.sha256)
                })
                .unwrap_or(false)
        {
            return Ok(witness.prefix);
        }
        // A declined probe is not an error or a new read authority. Rewind
        // through the original scan with the witness and its decode order.
        let mut first = Some(witness);
        self.scan_lines(|line, last, _| {
            if let Some(witness) = first.take()
                && line.len() == expected_bytes
                && ring::digest::digest(&ring::digest::SHA256, line).as_ref()
                    == witness.sha256.as_ref()
            {
                release_scan_line(line, last);
                prefix = witness.prefix;
                return Ok(());
            }
            // A different complete line still follows exactly the original
            // typed decode. Never turn a serde error into an EOF/count error.
            let record = serde_json::from_slice(line)?;
            release_scan_line(line, last);
            prefix.visit_owned_prefix(record);
            Ok(())
        })?;
        Ok(prefix)
    }

    fn scan_lines(
        &mut self,
        mut consume: impl FnMut(&mut Vec<u8>, bool, usize) -> Result<()>,
    ) -> Result<()> {
        self.file.rewind()?;
        let mut reader = BufReader::new(&mut self.file);
        let mut count = 0usize;
        let mut line = Vec::new();
        if self.single_record_reserve != 0 {
            // A hint only: reserve failure keeps the ordinary incremental path.
            let _ = line.try_reserve_exact(self.single_record_reserve);
        }
        while reader.read_until(b'\n', &mut line)? > 0 {
            anyhow::ensure!(
                line.ends_with(b"\n"),
                "incomplete transcript staging record"
            );
            consume(&mut line, count.checked_add(1) == Some(self.count), count)?;
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

fn matches_prefix_line(
    input: &mut impl Read,
    mut remaining: usize,
    expected_digest: &ring::digest::Digest,
) -> io::Result<bool> {
    let mut scratch = [0; PREFIX_WITNESS_CHUNK_BYTES];
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    loop {
        let len = remaining.min(scratch.len());
        let chunk = &mut scratch[..len];
        input.read_exact(chunk)?;
        remaining -= len;
        let body = if remaining == 0 {
            let Some(body) = chunk.strip_suffix(b"\n") else {
                return Ok(false);
            };
            body
        } else {
            &*chunk
        };
        if body.contains(&b'\n') {
            return Ok(false);
        }
        digest.update(chunk);
        if remaining == 0 {
            break;
        }
    }
    if digest.finish().as_ref() != expected_digest.as_ref() {
        return Ok(false);
    }
    // A matching first line isn't enough: there must be physical EOF.
    loop {
        match input.read(&mut scratch[..1]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => return result.map(|read| read == 0),
        }
    }
}

fn release_scan_line(line: &mut Vec<u8>, last: bool) {
    // A memory hint only: the same loop continues to physical EOF and count.
    if line.capacity() > MAX_RETAINED_FINAL_SCAN_BYTES && last {
        *line = Vec::new();
    }
}

fn staged_text_shape(h: &HistoryMutated) -> bool {
    h.mutation == HistoryMutationKind::Append
        && h.compaction.is_none()
        && h.tool_results.is_empty()
        && (MIN_ROWS..=MAX_ROWS).contains(&h.messages.len())
}

fn prefix_candidate(record: &JournalRecord) -> bool {
    matches!(
        &record.entry,
        JournalEntry::HistoryMutated(h)
            if staged_text_shape(h) && h.messages.iter().all(|message|
                matches!(message.parts.as_slice(),
                    [part] if matches!(part.payload, ContentPart::Text { .. })
                )
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::JOURNAL_SCHEMA_VERSION,
        domain::{new_record_id, new_session_id},
        model_standard::{CanonicalMessage, MessageRole},
    };

    #[test]
    fn only_large_bounded_single_records_get_a_reserve_hint() {
        let floor = MAX_RETAINED_FINAL_SCAN_BYTES as u64;
        for count in [0, 2, usize::MAX] {
            assert_eq!(single_record_reserve(count, floor + 1), 0);
        }
        for bytes in [0, floor, MAX_SINGLE_RECORD_RESERVE + 1, MAX_STAGING_BYTES] {
            assert_eq!(single_record_reserve(1, bytes), 0);
        }
        for bytes in [floor + 1, MAX_SINGLE_RECORD_RESERVE] {
            assert_eq!(single_record_reserve(1, bytes), bytes as usize);
        }
    }

    #[test]
    fn prefix_probe_bounds_reads_and_retries_short_or_interrupted_reads() {
        struct ShortReads<'a> {
            input: &'a [u8],
            interrupt: bool,
        }
        impl Read for ShortReads<'_> {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                assert!(bytes.len() <= PREFIX_WITNESS_CHUNK_BYTES);
                self.interrupt = !self.interrupt;
                if self.interrupt {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                let count = bytes.len().min(61);
                self.input.read(&mut bytes[..count])
            }
        }
        for size in [
            PREFIX_WITNESS_CHUNK_BYTES,
            PREFIX_WITNESS_CHUNK_BYTES + 1,
        ] {
            let mut line = vec![b'X'; size];
            line[size - 1] = b'\n';
            let sha = ring::digest::digest(&ring::digest::SHA256, &line);
            let mut reader = ShortReads { input: &line, interrupt: false };
            assert!(matches_prefix_line(&mut reader, size, &sha).unwrap());
            // Even a digest of the exact bytes cannot witness a second line.
            line[PREFIX_WITNESS_CHUNK_BYTES - 2] = b'\n';
            let sha = ring::digest::digest(&ring::digest::SHA256, &line);
            assert!(!matches_prefix_line(&mut line.as_slice(), size, &sha).unwrap());
        }
    }

    #[test]
    fn witnessed_large_stage_keeps_prefix_identities_and_fallback_errors() {
        let text = format!("{}é🦀\\\n", "x".repeat(980));
        let mut record = JournalRecord {
            schema_version: JOURNAL_SCHEMA_VERSION,
            record_id: new_record_id(),
            session_seq: 1,
            timestamp_ms: 42,
            session_id: new_session_id(),
            execution_id: None,
            thread_id: None,
            turn_id: None,
            entry: JournalEntry::HistoryMutated(HistoryMutated {
                previous_revision: 0,
                new_revision: 1,
                mutation: HistoryMutationKind::Append,
                messages: (0..MIN_ROWS)
                    .map(|_| CanonicalMessage::text(MessageRole::User, text.as_str()))
                    .collect(),
                compaction: None,
                tool_results: vec![],
            }),
        };
        let first = PartId::from_u128(1);
        let replacement = PartId::from_u128(2);
        let JournalEntry::HistoryMutated(mutation) = &mut record.entry else {
            unreachable!();
        };
        mutation.messages[0].parts[0].part_id = first;

        let mut writer = DisplayRecordWriter::new_for_replay().unwrap();
        writer.push(&record);
        let mut staged = writer.finish().unwrap();
        assert!(staged.single_record_reserve > MAX_RETAINED_FINAL_SCAN_BYTES);
        let size = staged.single_record_reserve;
        let mut exact = vec![0; size];
        staged.file.rewind().unwrap();
        staged.file.read_exact(&mut exact).unwrap();
        staged.scan_capturing_prefix(|_| Ok(())).unwrap();
        assert!(staged.prefix_witness.is_some());
        assert!(staged.scan_prefix().unwrap().has_explicit_part(&first));
        assert!(staged.prefix_witness.is_none());

        // Rearm from the original complete stage before doing any test writes.
        staged.scan_capturing_prefix(|_| Ok(())).unwrap();
        let JournalEntry::HistoryMutated(mutation) = &mut record.entry else {
            unreachable!();
        };
        mutation.messages[0].parts[0].part_id = replacement;
        let mut changed = serde_json::to_vec(&record).unwrap();
        changed.push(b'\n');
        assert_eq!(changed.len(), size);
        staged.file.rewind().unwrap();
        staged.file.write_all(&changed).unwrap();
        let prefix = staged.scan_prefix().unwrap();
        assert!(!prefix.has_explicit_part(&first));
        assert!(prefix.has_explicit_part(&replacement));
        drop(changed);

        // The stock decoder diagnoses the extra line before checking count.
        let bad = b"{not json}\n";
        let serde_error = serde_json::from_slice::<JournalRecord>(bad).unwrap_err().to_string();
        let JournalEntry::HistoryMutated(mutation) = &mut record.entry else {
            unreachable!();
        };
        mutation.messages.truncate(1);
        let mut valid_tail = serde_json::to_vec(&record).unwrap();
        valid_tail.push(b'\n');
        for (tail, truncated, error) in [
            (bad.as_slice(), false, serde_error.as_str()),
            (
                valid_tail.as_slice(),
                false,
                "transcript staging record count changed",
            ),
            (b"{}".as_slice(), false, "incomplete transcript staging record"),
            (b"".as_slice(), true, "incomplete transcript staging record"),
        ] {
            staged.file.set_len(size as u64).unwrap();
            staged.file.rewind().unwrap();
            staged.file.write_all(&exact).unwrap();
            staged.scan_capturing_prefix(|_| Ok(())).unwrap();
            assert!(staged.prefix_witness.is_some());
            if truncated {
                staged.file.set_len(size as u64 - 1).unwrap();
            } else {
                staged.file.seek(io::SeekFrom::End(0)).unwrap();
                staged.file.write_all(tail).unwrap();
            }
            assert_eq!(staged.scan_prefix().unwrap_err().to_string(), error);
            assert!(staged.prefix_witness.is_none());
        }
    }
}
