use std::{
    fs::OpenOptions as StdOpenOptions,
    io::{ErrorKind, Write},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use proteus_contracts::domain::{ExecutionId, SessionId, ThreadId, TurnId, new_record_id};
use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};

use super::{
    projection::{JournalProjection, JournalValidationState},
    types::{JOURNAL_SCHEMA_VERSION, JournalEntry, JournalKind, JournalRecord},
};

mod append;
mod initialize;
mod ownership;
pub(crate) use initialize::{initialize_writer_projection, initialize_writer_state};
mod read;
pub(crate) use append::append_record;
pub(crate) use read::load_records;
mod recovery;
mod redaction;
mod retirement;
pub(crate) use retirement::WriterRetirement;

pub(crate) use ownership::SessionWriteOwnership;
use recovery::{AppendRollback, restore_committed_offset};
pub(super) use redaction::contains_redacted_sensitive_value;
use redaction::redact_sensitive_values;
pub(crate) use redaction::redacted_history;

pub const JOURNAL_FILE: &str = "journal.jsonl";
const BLOBS_DIR: &str = "blobs";
pub const DEFAULT_BLOB_THRESHOLD_BYTES: usize = 256 * 1024;
const MAX_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredJournalRecord {
    schema_version: u32,
    record_id: proteus_contracts::domain::RecordId,
    session_seq: u64,
    timestamp_ms: i64,
    session_id: SessionId,
    execution_id: Option<ExecutionId>,
    thread_id: Option<ThreadId>,
    turn_id: Option<TurnId>,
    kind: JournalKind,
    payload: StoredPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "storage", rename_all = "snake_case", deny_unknown_fields)]
enum StoredPayload {
    Inline {
        value: serde_json::Value,
    },
    Blob {
        sha256: String,
        bytes: u64,
        relative_path: PathBuf,
    },
}

#[derive(Debug, Default)]
pub(crate) struct JournalWriterState {
    initialized: bool,
    retired: bool,
    usage: super::UsageProjection,
    model_context: crate::core::model_context::projection::ModelContextProjection,
    next_seq: u64,
    validation: JournalValidationState,
    committed_offset: u64,
    _ownership: Option<SessionWriteOwnership>,
    #[cfg(test)]
    initial_recovery_scans: usize,
    #[cfg(test)]
    append_pause: Option<append::AppendPause>,
    // Fields drop in declaration order. Signal only after validation buffers,
    // the OS lease and any test IO pause have been released.
    _retirement: Option<retirement::RetirementGuard>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct JournalRecordAttribution {
    pub execution_id: Option<ExecutionId>,
    pub thread_id: Option<ThreadId>,
    pub turn_id: Option<TurnId>,
}

impl JournalRecordAttribution {
    pub fn chat(thread_id: ThreadId, turn_id: Option<TurnId>) -> Self {
        Self {
            execution_id: None,
            thread_id: Some(thread_id),
            turn_id,
        }
    }

    pub fn execution(
        execution_id: ExecutionId,
        thread_id: Option<ThreadId>,
        turn_id: Option<TurnId>,
    ) -> Self {
        Self {
            execution_id: Some(execution_id),
            thread_id,
            turn_id,
        }
    }
}

impl JournalWriterState {
    /// Release decoded execution state without releasing the sole-writer lease.
    pub(crate) fn hibernate(&mut self) {
        self.initialized = false;
        self.validation = Default::default();
        self.usage = Default::default();
        self.model_context = Default::default();
    }

    /// Newly acquired ownership remains tentative until preflight succeeds.
    /// A rejected preflight must not strand a lease in a still-live read handle.
    /// Existing ownership is never released by a rejected later operation.
    pub(crate) fn with_ownership_preflight<T>(
        &mut self,
        session_dir: &Path,
        session_id: SessionId,
        preflight: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        self.ensure_active()?;
        let newly_acquired = self._ownership.is_none();
        if newly_acquired {
            self._ownership = Some(SessionWriteOwnership::acquire(session_dir, session_id)?);
        }
        struct Tentative<'a> {
            state: &'a mut JournalWriterState,
            newly_acquired: bool,
            committed: bool,
        }
        impl Drop for Tentative<'_> {
            fn drop(&mut self) {
                if self.newly_acquired && !self.committed {
                    self.state._ownership.take();
                }
            }
        }
        let mut tentative = Tentative {
            state: self,
            newly_acquired,
            committed: false,
        };
        let result = preflight(&mut *tentative.state);
        tentative.committed = result.is_ok();
        result
    }

    pub(crate) fn ensure_active(&self) -> Result<()> {
        anyhow::ensure!(!self.retired, "session writer generation is retired");
        Ok(())
    }

    /// Transfer the actual lease and completion tail, never unlock/reacquire.
    /// The shared generation remains terminal even if filesystem deletion fails.
    pub(crate) fn retire_for_delete(&mut self) -> Result<impl Send + use<>> {
        self.ensure_active()?;
        self.retired = true;
        // Tuple fields drop in order: OS lease closes before completion is signaled.
        Ok((self._ownership.take(), self._retirement.take()))
    }

    pub(crate) fn with_retirement(retirement: std::sync::Arc<WriterRetirement>) -> Self {
        Self {
            _retirement: Some(retirement::RetirementGuard(retirement)),
            ..Self::default()
        }
    }

    /// Hold the destructor's lease/completion tail separately to exercise the
    /// interval after the final Arc expires but before writer teardown ends.
    #[cfg(test)]
    pub(crate) fn detach_retirement(&mut self) -> impl Send + use<> {
        (self._ownership.take(), self._retirement.take())
    }

    pub(crate) fn model_context_before_turn(
        &self,
        turn_id: TurnId,
    ) -> Result<Option<Vec<crate::contracts::ModelContextObservation>>> {
        if !self.initialized {
            return Ok(None);
        }
        self.model_context
            .before_turn(turn_id)
            .map(Some)
            .ok_or_else(|| anyhow!("model context requested outside the latest admitted turn"))
    }

    pub(crate) fn committed_history(
        &self,
        thread_id: ThreadId,
        turn_id: TurnId,
    ) -> Option<(Vec<crate::model_standard::CanonicalMessage>, usize)> {
        self.initialized
            .then(|| self.validation.committed_history(thread_id, turn_id))
    }

    pub(crate) fn usage_snapshot(
        &self,
        session_id: SessionId,
    ) -> Option<crate::domain::SessionUsageSnapshot> {
        self.initialized.then(|| self.usage.snapshot(session_id))
    }

    pub(crate) fn cached_history_revision(&self) -> Option<u64> {
        self.initialized.then(|| self.history_revision())
    }

    pub(crate) fn execution_state(
        &self,
    ) -> Option<(u64, Vec<crate::contracts::WorkflowHistoryInterruption>)> {
        self.initialized
            .then(|| (self.history_revision(), self.validation.interrupted_turns()))
    }

    pub(crate) fn history_revision(&self) -> u64 {
        self.validation.history_revision()
    }

    #[cfg(test)]
    pub(crate) fn initial_recovery_scans(&self) -> usize {
        self.initial_recovery_scans
    }
}

pub(crate) fn journal_path(session_dir: &Path) -> PathBuf {
    session_dir.join(JOURNAL_FILE)
}

fn journal_len(path: &Path) -> Result<u64> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error).with_context(|| format!("failed to inspect {}", path.display())),
    }
}

fn write_blob(session_dir: &Path, bytes: &[u8]) -> Result<StoredPayload> {
    let sha256 = sha256_hex(bytes);
    let relative_path = PathBuf::from(BLOBS_DIR).join(format!("{sha256}.json"));
    let path = session_dir.join(&relative_path);
    std::fs::create_dir_all(session_dir.join(BLOBS_DIR))?;
    if path.try_exists()? {
        let existing = std::fs::read(&path)?;
        if existing != bytes {
            bail!("content-addressed blob collision at {}", path.display());
        }
    } else {
        let tmp_path = session_dir
            .join(BLOBS_DIR)
            .join(format!(".{sha256}.tmp.{}", new_record_id()));
        let mut file = StdOpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp_path)
            .with_context(|| format!("failed to create {}", tmp_path.display()))?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_data()?;
        match std::fs::rename(&tmp_path, &path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                let existing = std::fs::read(&path)?;
                if existing != bytes {
                    bail!("content-addressed blob collision at {}", path.display());
                }
                let _ = std::fs::remove_file(&tmp_path);
            }
            Err(error) => {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(error)
                    .with_context(|| format!("failed to install blob {}", path.display()));
            }
        }
    }
    Ok(StoredPayload::Blob {
        sha256,
        bytes: bytes.len() as u64,
        relative_path,
    })
}

fn hydrate_entry(
    session_dir: &Path,
    kind: JournalKind,
    payload: StoredPayload,
) -> Result<JournalEntry> {
    match payload {
        StoredPayload::Inline { value } => {
            let bytes = serde_json::to_vec(&value)?.len();
            if bytes > MAX_PAYLOAD_BYTES {
                bail!(
                    "inline journal payload is {bytes} bytes, maximum is {MAX_PAYLOAD_BYTES} bytes"
                );
            }
            Ok(JournalEntry::from_kind_and_payload(kind, value)?)
        }
        StoredPayload::Blob {
            sha256,
            bytes,
            relative_path,
        } => {
            validate_blob_reference(&sha256, &relative_path)?;
            if bytes > MAX_PAYLOAD_BYTES as u64 {
                bail!("journal blob declares {bytes} bytes, maximum is {MAX_PAYLOAD_BYTES} bytes");
            }
            let path = session_dir.join(relative_path);
            let actual_bytes = std::fs::metadata(&path)
                .with_context(|| format!("failed to inspect journal blob {}", path.display()))?
                .len();
            if actual_bytes != bytes {
                bail!(
                    "journal blob {} size mismatch: expected {}, found {}",
                    path.display(),
                    bytes,
                    actual_bytes
                );
            }
            let content = std::fs::read(&path)
                .with_context(|| format!("failed to read journal blob {}", path.display()))?;
            let actual = sha256_hex(&content);
            if actual != sha256 {
                bail!(
                    "journal blob {} hash mismatch: expected {}, found {}",
                    path.display(),
                    sha256,
                    actual
                );
            }
            JournalEntry::from_kind_and_bytes(kind, &content)
                .with_context(|| format!("journal blob {} has an invalid payload", path.display()))
        }
    }
}

fn validate_blob_reference(sha256: &str, relative_path: &Path) -> Result<()> {
    if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid journal blob sha256 '{sha256}'");
    }
    let components = relative_path.components().collect::<Vec<_>>();
    let expected_name = format!("{}.json", sha256.to_ascii_lowercase());
    match components.as_slice() {
        [Component::Normal(dir), Component::Normal(file)]
            if *dir == std::ffi::OsStr::new(BLOBS_DIR)
                && *file == std::ffi::OsStr::new(&expected_name) =>
        {
            Ok(())
        }
        _ => Err(anyhow!(
            "journal blob path must be blobs/<sha256>.json, found {}",
            relative_path.display()
        )),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    digest(&SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn unix_timestamp_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod read_tests;
