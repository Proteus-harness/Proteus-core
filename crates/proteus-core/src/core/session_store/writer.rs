use super::*;
use crate::contracts::WorkflowToolResultBinding;

impl SessionStore {
    pub(crate) async fn hibernate_writer(&self) {
        let mut writer = self.writer.lock().await;
        #[cfg(unix)]
        if let (Some(revision), Some(usage)) = (
            writer.cached_history_revision(),
            writer.usage_snapshot(self.session_id),
        ) {
            self.set_read_metadata(revision, usage);
        }
        writer.hibernate();
    }

    /// The prefix is captured when TurnOpened commits. Later facts, including
    /// late responses from another execution, cannot change this turn's input.
    pub(crate) async fn model_context_before_turn(
        &self,
        turn_id: TurnId,
    ) -> Result<Vec<crate::contracts::ModelContextObservation>> {
        {
            let writer = self.writer.lock().await;
            writer.ensure_active()?;
            if let Some(context) = writer.model_context_before_turn(turn_id)? {
                return Ok(context);
            }
        }
        self.write_transaction(move |_, writer| {
            writer
                .model_context_before_turn(turn_id)?
                .ok_or_else(|| anyhow!("writer was not initialized"))
        })
        .await
    }

    /// Reconcile acknowledged or lost-ack checkpoints from the sole writer's
    /// validated state, without replaying every previous request and tool output.
    pub(crate) async fn committed_history(
        &self,
        thread_id: ThreadId,
        turn_id: TurnId,
    ) -> Result<(Vec<CanonicalMessage>, usize)> {
        {
            let writer = self.writer.lock().await;
            writer.ensure_active()?;
            if let Some(state) = writer.committed_history(thread_id, turn_id) {
                return Ok(state);
            }
        }
        self.write_transaction(move |_, writer| {
            writer
                .committed_history(thread_id, turn_id)
                .ok_or_else(|| anyhow!("writer was not initialized"))
        })
        .await
    }

    /// Execution reads the projection already validated by the sole writer.
    /// Cold initialization still acquires ownership and validates on a worker.
    pub(crate) async fn execution_state(
        &self,
    ) -> Result<(u64, Vec<crate::contracts::WorkflowHistoryInterruption>)> {
        {
            let writer = self.writer.lock().await;
            writer.ensure_active()?;
            if let Some(state) = writer.execution_state() {
                return Ok(state);
            }
        }
        self.write_transaction(|_, writer| {
            writer
                .execution_state()
                .ok_or_else(|| anyhow!("writer was not initialized"))
        })
        .await
    }

    /// First continuation acquires the existing writer lease and reuses its strict
    /// recovery projection. Display caches never initialize writer validation.
    pub(crate) async fn prepare_resume_projection(&self) -> Result<JournalProjection> {
        let mut writer = self.writer.clone().lock_owned().await;
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            writer.ensure_active()?;
            store.prepare_lease_parent()?;
            writer.with_ownership_preflight(&store.session_dir, store.session_id, |writer| {
                store.materialize_for_write()?;
                match crate::core::session_journal::initialize_writer_projection(
                    &store.session_dir,
                    store.session_id,
                    writer,
                )? {
                    Some(projection) => Ok(projection),
                    None => store.load_projection(),
                }
            })
        })
        .await
        .context("session resume validation worker failed")?
    }

    #[cfg(test)]
    pub(crate) async fn pause_next_append(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        std::sync::mpsc::Sender<bool>,
    ) {
        self.writer.lock().await.pause_next_append()
    }

    /// Once admitted, the worker owns the lock until commit or rollback. Dropping
    /// the caller only stops waiting; it cannot interrupt a durable transaction.
    async fn write_transaction<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&Self, &mut JournalWriterState) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let mut writer = self.writer.clone().lock_owned().await;
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            writer.ensure_active()?;
            store.prepare_lease_parent()?;
            writer.with_ownership_preflight(&store.session_dir, store.session_id, |writer| {
                store.materialize_for_write()?;
                initialize_writer_state(&store.session_dir, store.session_id, writer)
            })?;
            // Ownership is committed before the requested operation. A rejected
            // append must not discard an already valid writer's lease.
            operation(&store, &mut writer)
        })
        .await
        .context("session journal transaction worker failed")?
    }

    /// Admission may write image bytes before the first journal record. Create
    /// the canonical session identity and acquire its writer lease first.
    pub(crate) async fn prepare_attachments(&self) -> Result<()> {
        self.write_transaction(|_, _| Ok(())).await
    }

    fn prepare_lease_parent(&self) -> Result<()> {
        let parent = self
            .session_dir
            .parent()
            .context("session directory has no parent")?;
        if self.may_create.load(std::sync::atomic::Ordering::SeqCst) {
            // Only ancestors, never the session or its metadata, precede the lease.
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create session parent {}", parent.display()))?;
        }
        Ok(())
    }

    fn materialize_for_write(&self) -> Result<()> {
        use std::sync::atomic::Ordering;
        let created = match std::fs::symlink_metadata(&self.session_dir) {
            Ok(metadata) => {
                self.may_create.store(false, Ordering::SeqCst);
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    anyhow::bail!(
                        "session path is not a directory: {}",
                        self.session_dir.display()
                    );
                }
                false
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !self.may_create.load(Ordering::SeqCst) {
                    anyhow::bail!(
                        "existing session is missing: {}",
                        self.session_dir.display()
                    );
                }
                match std::fs::create_dir(&self.session_dir) {
                    Ok(()) => {
                        // Consumed even if the subsequent metadata write fails.
                        self.may_create.store(false, Ordering::SeqCst);
                        true
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        self.may_create.store(false, Ordering::SeqCst);
                        false
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!(
                                "failed to create session dir {}",
                                self.session_dir.display()
                            )
                        });
                    }
                }
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to inspect session dir {}",
                        self.session_dir.display()
                    )
                });
            }
        };
        if !created {
            let metadata = std::fs::symlink_metadata(&self.session_dir)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                anyhow::bail!(
                    "session path is not a directory: {}",
                    self.session_dir.display()
                );
            }
            let identity = resolve_session_identity(&self.session_dir)?;
            if identity.session_id != self.session_id {
                anyhow::bail!(
                    "session identity changed: expected {}, found {}",
                    self.session_id,
                    identity.session_id
                );
            }
            return Ok(());
        }
        ensure_writable_identity(
            &self.session_dir,
            self.session_id,
            self.directory_kind,
            &self.workspace_path()?,
            true,
        )
    }

    pub(super) async fn append_fact(
        &self,
        attribution: JournalRecordAttribution,
        entry: JournalEntry,
    ) -> Result<JournalRecord> {
        self.write_transaction(move |store, writer| {
            append_record(
                &store.session_dir,
                store.session_id,
                attribution,
                entry,
                store.blob_threshold_bytes,
                writer,
            )
        })
        .await
    }

    pub(super) async fn mutate_history(
        &self,
        thread_id: ThreadId,
        turn_id: Option<TurnId>,
        messages: Vec<CanonicalMessage>,
        mutation: HistoryMutationKind,
        compaction: Option<HistoryCompactionReport>,
        tool_results: Vec<WorkflowToolResultBinding>,
    ) -> Result<()> {
        self.write_transaction(move |store, writer| {
            let previous_revision = writer.history_revision();
            append_record(
                &store.session_dir,
                store.session_id,
                JournalRecordAttribution::chat(thread_id, turn_id),
                JournalEntry::HistoryMutated(HistoryMutated {
                    previous_revision,
                    new_revision: previous_revision.saturating_add(1),
                    tool_results,
                    mutation,
                    messages,
                    compaction,
                }),
                store.blob_threshold_bytes,
                writer,
            )?;
            Ok(())
        })
        .await
    }
}
