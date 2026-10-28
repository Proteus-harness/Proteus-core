use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex, OnceLock, Weak},
};

use anyhow::{Context, Result, anyhow};
use tokio::sync::Mutex;

use crate::{
    contracts::ExecutionAttribution,
    core::session_journal::{
        DEFAULT_BLOB_THRESHOLD_BYTES, HistoryMutated, HistoryMutationKind, JournalEntry,
        JournalProjection, JournalRecord, JournalRecordAttribution, JournalWriterState,
        WriterRetirement, append_record, initialize_writer_state, journal_path, load_records,
    },
    domain::{ExecutionId, HistoryCompactionReport, SessionId, ThreadId, TurnId},
    model_standard::CanonicalMessage,
};

#[cfg(test)]
use crate::model_standard::{ContentPart, MessageRole};

mod catalog;
mod checkpoint;
mod context_summary;
mod deletion;
pub(crate) use context_summary::summarize_context_history;
mod identity;
mod path_identity;
#[cfg(unix)]
pub(crate) mod read_cache;
mod read_model;
mod usage;
mod workspace_dir;
mod writer;

#[cfg(unix)]
pub(crate) use catalog::subscribe_catalog_updates;
pub(crate) use catalog::{PreparedSession, prepare_latest_workspace_session};
pub use catalog::{
    list_session_summaries, list_session_summaries_for_audit, list_workspace_session_summaries,
};
use identity::{
    SessionDirectoryKind, ensure_writable_identity, resolve_session_identity,
    short_session_directory_name, validate_new_session_target,
};
use path_identity::canonicalize_unmaterialized_path;
pub(crate) use read_model::SessionReadModel;
use workspace_dir::workspace_path_from_session_dir;
pub use workspace_dir::{decode_workspace_path, encode_workspace_path};

#[derive(Debug, Clone)]
pub struct SessionStore {
    session_dir: PathBuf,
    session_id: SessionId,
    directory_kind: SessionDirectoryKind,
    writer: Arc<Mutex<JournalWriterState>>,
    // Permission originates in the constructor's single initial observation.
    // Clones share it; the first mkdir/observed existing slot consumes it.
    may_create: Arc<std::sync::atomic::AtomicBool>,
    blob_threshold_bytes: usize,
    #[cfg(unix)]
    read_metadata: Arc<std::sync::RwLock<Option<read_model::CachedReadMetadata>>>,
}

impl SessionStore {
    pub fn new(config_dir: &Path, cwd: &Path, session_id: SessionId) -> Result<Self> {
        let workspace = encode_workspace_path(cwd)?;
        let session_dir = canonicalize_unmaterialized_path(
            &config_dir
                .join("sessions")
                .join(workspace)
                .join(short_session_directory_name(session_id)),
        )?;
        let may_create = validate_new_session_target(&session_dir, session_id)?;
        let writer = writer_for_session_dir(&session_dir)?;
        Ok(Self {
            session_dir,
            session_id,
            directory_kind: SessionDirectoryKind::ShortNumeric,
            writer,
            may_create: Arc::new(std::sync::atomic::AtomicBool::new(may_create)),
            blob_threshold_bytes: DEFAULT_BLOB_THRESHOLD_BYTES,
            #[cfg(unix)]
            read_metadata: Default::default(),
        })
    }

    pub fn open(session_dir: PathBuf) -> Result<Self> {
        let session_dir = canonicalize_unmaterialized_path(&session_dir)?;
        let identity = resolve_session_identity(&session_dir)?;
        workspace_path_from_session_dir(&session_dir)?;
        let writer = writer_for_session_dir(&session_dir)?;
        Ok(Self {
            session_dir,
            session_id: identity.session_id,
            directory_kind: identity.directory_kind,
            writer,
            may_create: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            blob_threshold_bytes: DEFAULT_BLOB_THRESHOLD_BYTES,
            #[cfg(unix)]
            read_metadata: Default::default(),
        })
    }

    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub fn workspace_path(&self) -> Result<PathBuf> {
        workspace_path_from_session_dir(&self.session_dir)
    }

    pub fn journal_path(&self) -> PathBuf {
        journal_path(&self.session_dir)
    }

    pub fn load_messages(&self) -> Result<Vec<CanonicalMessage>> {
        Ok(self.load_projection()?.history)
    }

    /// Read the current canonical conversation, without replay or a writer lease.
    pub fn conversation_snapshot(&self) -> Result<crate::contracts::ConversationSnapshot> {
        #[cfg(unix)]
        if let Some(conversation) = read_model::sqlite_conversation(self) {
            return Ok(conversation);
        }
        let projection = self.load_projection()?;
        let model_context =
            super::model_context::ModelContextState::from_records(&projection.records, None)
                .snapshot();
        Ok(crate::contracts::ConversationSnapshot {
            revision: projection.history_revision,
            messages: projection.history,
            model_context,
        })
    }

    pub fn load_records(&self) -> Result<Vec<JournalRecord>> {
        load_records(&self.session_dir, self.session_id)
    }

    /// Read the committed writer revision without scanning every record on each hook.
    /// A read-only cold store has not acquired a writer lease; use its projection then.
    pub(crate) async fn history_revision(&self) -> Result<u64> {
        if let Some(revision) = self.writer.lock().await.cached_history_revision() {
            return Ok(revision);
        }
        #[cfg(unix)]
        if let Some(cached) = self.cached_read_metadata() {
            return Ok(cached.revision);
        }
        Ok(self.load_projection()?.history_revision)
    }

    pub fn load_projection(&self) -> Result<JournalProjection> {
        JournalProjection::build(self.session_id, self.load_records()?)
    }

    pub async fn append_history(
        &self,
        thread_id: ThreadId,
        turn_id: Option<TurnId>,
        messages: &[CanonicalMessage],
    ) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        self.mutate_history(
            thread_id,
            turn_id,
            messages.to_vec(),
            HistoryMutationKind::Append,
            None,
            Vec::new(),
        )
        .await
    }

    pub async fn replace_history(
        &self,
        thread_id: ThreadId,
        turn_id: Option<TurnId>,
        messages: &[CanonicalMessage],
        compaction: Option<HistoryCompactionReport>,
    ) -> Result<()> {
        self.mutate_history(
            thread_id,
            turn_id,
            messages.to_vec(),
            HistoryMutationKind::Replace,
            compaction,
            Vec::new(),
        )
        .await
    }

    pub async fn append_journal_entry(
        &self,
        thread_id: ThreadId,
        turn_id: Option<TurnId>,
        entry: JournalEntry,
    ) -> Result<JournalRecord> {
        self.append_fact(JournalRecordAttribution::chat(thread_id, turn_id), entry)
            .await
    }

    pub async fn append_execution_journal_entry(
        &self,
        attribution: ExecutionAttribution,
        entry: JournalEntry,
    ) -> Result<JournalRecord> {
        let (thread_id, turn_id) = match attribution.agent {
            Some(agent) => {
                if agent.session_id != self.session_id {
                    anyhow::bail!(
                        "execution attribution belongs to session {}, recorder store belongs to {}",
                        agent.session_id,
                        self.session_id
                    );
                }
                (Some(agent.thread_id), Some(agent.turn_id))
            }
            None => (None, None),
        };
        self.append_execution_fact(attribution.execution_id, thread_id, turn_id, entry)
            .await
    }

    async fn append_execution_fact(
        &self,
        execution_id: ExecutionId,
        thread_id: Option<ThreadId>,
        turn_id: Option<TurnId>,
        entry: JournalEntry,
    ) -> Result<JournalRecord> {
        self.append_fact(
            JournalRecordAttribution::execution(execution_id, thread_id, turn_id),
            entry,
        )
        .await
    }

    pub async fn clear_history(&self, thread_id: ThreadId) -> Result<()> {
        self.replace_history(thread_id, None, &[], None).await
    }
}

/// Conservatively refuse deletion while any cooperating process owns a writer.
/// Callers must stop work and drop all owner handles before retrying. This does
/// not implement coordinated active-session shutdown or local retirement.
pub async fn delete_workspace_session(
    config_root: &Path,
    workspace_path: &Path,
    session_path: PathBuf,
) -> Result<bool> {
    let config_root = config_root.to_path_buf();
    let workspace_path = workspace_path.to_path_buf();
    // The closure owns both deletion and its lease. Canceling the waiter cannot
    // release the lease while remove_dir_all is still running.
    tokio::task::spawn_blocking(move || {
        let session_dir = normalize_session_dir_path(session_path)?;
        let metadata = match std::fs::symlink_metadata(&session_dir) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect {}", session_dir.display()));
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(anyhow!(
                "session path is not a directory: {}",
                session_dir.display()
            ));
        }
        let workspace_dir = config_root
            .join("sessions")
            .join(encode_workspace_path(&workspace_path)?);
        let workspace_root = std::fs::canonicalize(&workspace_dir)
            .with_context(|| format!("failed to resolve {}", workspace_dir.display()))?;
        let target = std::fs::canonicalize(&session_dir)
            .with_context(|| format!("failed to resolve {}", session_dir.display()))?;
        if target.parent() != Some(workspace_root.as_path()) {
            return Err(anyhow!(
                "session path is outside current workspace sessions: {}",
                session_dir.display()
            ));
        }
        let _ownership =
            crate::core::session_journal::SessionWriteOwnership::acquire_slot(&target)?;
        // Validate again under the lease; a cold handle never grants identity.
        match std::fs::symlink_metadata(&target) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(anyhow!(
                    "session path is not a directory: {}",
                    target.display()
                ));
            }
            Ok(_) => {}
        }
        resolve_session_identity(&target)?;
        std::fs::remove_dir_all(&target)
            .with_context(|| format!("failed to delete {}", target.display()))?;
        Ok(true)
    })
    .await
    .context("session deletion worker failed")?
}

pub fn normalize_session_dir_path(session_path: PathBuf) -> Result<PathBuf> {
    if session_path.file_name().and_then(|name| name.to_str())
        == Some(crate::core::session_journal::JOURNAL_FILE)
    {
        return session_path
            .parent()
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("journal.jsonl path has no parent session dir"));
    }
    Ok(session_path)
}

pub fn canonicalize_session_dir_path(session_path: PathBuf) -> Result<PathBuf> {
    let session_dir = normalize_session_dir_path(session_path)?;
    canonicalize_unmaterialized_path(&session_dir)
}

struct RegisteredWriter {
    writer: Weak<Mutex<JournalWriterState>>,
    retirement: Arc<WriterRetirement>,
}

fn writer_for_session_dir(session_dir: &Path) -> Result<Arc<Mutex<JournalWriterState>>> {
    static WRITERS: OnceLock<StdMutex<HashMap<PathBuf, RegisteredWriter>>> = OnceLock::new();
    let parent = session_dir
        .parent()
        .context("session writer slot has no parent")?;
    let name = session_dir
        .file_name()
        .context("session writer slot has no basename")?;
    let key = canonicalize_unmaterialized_path(parent)?.join(name);
    let writers = WRITERS.get_or_init(|| StdMutex::new(HashMap::new()));
    loop {
        let mut writers = writers.lock().expect("session journal writer map poisoned");
        if let Some(registered) = writers.get(&key) {
            if let Some(writer) = registered.writer.upgrade() {
                return Ok(writer);
            }
            if !registered.retirement.completed() {
                let retirement = registered.retirement.clone();
                drop(writers);
                // A zero strong count precedes field destruction. Wait for the
                // old lease to close without blocking unrelated session opens.
                retirement.wait();
                continue;
            }
        }
        // Stores and admitted transactions own live validation state and the OS
        // lease. Retiring generations keep their completion latch until done.
        writers.retain(|_, registered| {
            registered.writer.strong_count() > 0 || !registered.retirement.completed()
        });
        let retirement = Arc::new(WriterRetirement::default());
        let writer = Arc::new(Mutex::new(JournalWriterState::with_retirement(
            retirement.clone(),
        )));
        writers.insert(
            key,
            RegisteredWriter {
                writer: Arc::downgrade(&writer),
                retirement,
            },
        );
        return Ok(writer);
    }
}

#[cfg(test)]
mod cancellation_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod tests;
