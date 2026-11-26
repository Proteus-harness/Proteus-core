use super::*;
use proteus_contracts::{
    contracts::{
        ExecutionAttribution, SessionStorageFailure, StorageCommit, StorageRecordAttribution,
        StorageRevision, StorageSnapshot, StorageWriterLease,
    },
    domain::new_record_id,
    storage::journal::{HistoryMutated, HistoryMutationKind, JournalEntry, JournalRecord},
};

use super::{
    snapshot::{DisplayRecordWriter, is_display_kind},
    view::CommittedDisplay,
};

struct DisplayCapture {
    start: StorageSnapshot,
    records: DisplayRecordWriter,
}

fn lease_lost(error: &anyhow::Error) -> bool {
    error.downcast_ref::<SessionStorageFailure>()
        == Some(&SessionStorageFailure::InvalidWriterLease)
        || matches!(
            error
                .downcast_ref::<crate::process_adapters::ProcessInvocationError>()
                .map(crate::process_adapters::ProcessInvocationError::failure),
            Some(crate::process_adapters::ProcessInvocationFailure::ComponentLost(_))
        )
}

pub(super) struct RemoteWriter {
    service: StorageService,
    session: proteus_contracts::contracts::SessionLocator,
    lease: Option<StorageWriterLease>,
    revision: StorageRevision,
    retired: bool,
    display_capture: Option<DisplayCapture>,
}

// A newly opened read handle and a live runtime can address the same session.
pub(super) struct SessionHandles {
    writer: Arc<tokio::sync::Mutex<RemoteWriter>>,
    pub(super) publishing: std::sync::Mutex<Option<Arc<view::PublishingTranscript>>>,
}

pub(super) fn shared_handles(
    service: &StorageService,
    session: &proteus_contracts::contracts::SessionLocator,
) -> Arc<SessionHandles> {
    use std::{
        collections::HashMap,
        sync::{Mutex, OnceLock, Weak},
    };
    type Key = (usize, proteus_contracts::contracts::SessionLocator);
    type Handle = Weak<SessionHandles>;
    static WRITERS: OnceLock<Mutex<HashMap<Key, Handle>>> = OnceLock::new();
    let mut writers = WRITERS
        .get_or_init(Default::default)
        .lock()
        .expect("storage writer registry poisoned");
    let key = (Arc::as_ptr(service) as *const () as usize, session.clone());
    if let Some(writer) = writers.get(&key).and_then(Weak::upgrade) {
        return writer;
    }
    writers.retain(|_, writer| writer.strong_count() != 0);
    let writer = Arc::new(SessionHandles {
        writer: Arc::new(tokio::sync::Mutex::new(RemoteWriter::new(
            service.clone(),
            session.clone(),
        ))),
        publishing: Default::default(),
    });
    writers.insert(key, Arc::downgrade(&writer));
    writer
}

impl RemoteWriter {
    pub(super) fn new(
        service: StorageService,
        session: proteus_contracts::contracts::SessionLocator,
    ) -> Self {
        Self {
            service,
            session,
            lease: None,
            revision: StorageRevision::default(),
            retired: false,
            display_capture: None,
        }
    }

    async fn acquire(&mut self) -> Result<StorageWriterLease> {
        ensure!(!self.retired, "session writer has been retired");
        if let Some(lease) = &self.lease {
            return Ok(lease.clone());
        }
        self.display_capture = None;
        match self
            .service
            .execute(Request::AcquireWriter {
                session: self.session.clone(),
            })
            .await?
        {
            Response::Writer { lease, revision } => {
                ensure!(
                    lease.session == self.session,
                    "storage writer belongs to a different session"
                );
                self.revision = revision;
                self.lease = Some(lease.clone());
                Ok(lease)
            }
            _ => bail!("storage returned an unexpected writer response"),
        }
    }

    async fn read_state(
        &mut self,
        request: impl Fn(StorageWriterLease) -> Request,
    ) -> Result<Response> {
        let lease = self.acquire().await?;
        match self.service.execute(request(lease)).await {
            Err(error) if lease_lost(&error) => {
                self.lease = None;
                let lease = self.acquire().await?;
                self.service.execute(request(lease)).await
            }
            result => result,
        }
    }

    async fn display_source(&self) -> Option<StorageSnapshot> {
        let Response::Snapshot { snapshot } = self
            .service
            .execute(Request::OpenSnapshot {
                session: self.session.clone(),
            })
            .await
            .ok()?
        else {
            return None;
        };
        self.service
            .execute(Request::ReleaseSnapshot {
                snapshot: snapshot.clone(),
            })
            .await
            .ok()?;
        (snapshot.session == self.session && snapshot.revision == self.revision).then_some(snapshot)
    }

    async fn commit(
        &mut self,
        attribution: StorageRecordAttribution,
        entry: JournalEntry,
    ) -> Result<JournalRecord> {
        let lease = self.acquire().await?;
        let operation_id = new_record_id();
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis()
            .try_into()?;
        let commit = StorageCommit {
            lease,
            operation_id,
            expected: self.revision,
            timestamp_ms,
            attribution,
            entry,
        };
        let kind = commit.entry.kind();
        let result = self.service.execute(Request::Commit { commit }).await;
        let ack = match result {
            Ok(Response::Committed { ack }) => ack,
            Ok(_) => bail!("storage returned an unexpected commit response"),
            Err(original) => {
                self.display_capture = None;
                // Transport loss does not establish rollback. Reconcile the same
                // operation; never turn uncertain persistence into repeated tools.
                let status = self
                    .service
                    .execute(Request::CommitStatus {
                        session: self.session.clone(),
                        operation_id,
                    })
                    .await;
                if lease_lost(&original) {
                    self.lease = None;
                }
                match status {
                    Ok(Response::CommitStatus { ack: Some(ack) }) => ack,
                    _ => {
                        return Err(original.context("storage commit outcome was not acknowledged"));
                    }
                }
            }
        };
        ensure!(
            ack.operation_id == operation_id
                && ack.record.record_id == operation_id
                && ack.record.session_id == self.session.session_id,
            "storage commit acknowledgement identity mismatch"
        );
        ensure!(
            ack.record.session_seq
                == self
                    .revision
                    .session_seq
                    .checked_add(1)
                    .ok_or_else(|| anyhow!("storage sequence overflow"))?
                && ack.revision.session_seq == ack.record.session_seq
                && ack.revision.history_revision >= self.revision.history_revision,
            "storage commit acknowledgement revision mismatch"
        );
        ensure!(
            ack.record.execution_id == attribution.execution_id
                && ack.record.thread_id == attribution.thread_id
                && ack.record.turn_id == attribution.turn_id
                && ack.record.entry.kind() == kind,
            "storage commit acknowledgement attribution mismatch"
        );
        self.revision = ack.revision;
        if let Some(capture) = &mut self.display_capture
            && is_display_kind(kind)
        {
            capture.records.push(&ack.record);
        }
        if matches!(
            &ack.record.entry,
            JournalEntry::TurnSettled(_) | JournalEntry::HistoryMutated(HistoryMutated { .. })
        ) {
            super::catalog::notify_update();
        }
        Ok(ack.record)
    }
}

impl Drop for RemoteWriter {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else {
            return;
        };
        // A store without a writer never dispatches release or acquires a lease.
        let _ = self
            .service
            .execute_blocking(Request::ReleaseWriter { lease });
    }
}

impl RemoteSessionStore {
    async fn transaction<T: Send + 'static>(
        &self,
        operation: impl for<'a> FnOnce(
            &'a mut RemoteWriter,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<T>> + Send + 'a>,
        > + Send
        + 'static,
    ) -> Result<T> {
        let mut writer = self.handles.writer.clone().lock_owned().await;
        // Admission transfers ownership to this task. Caller cancellation stops
        // waiting while the admitted persistence transaction finishes/reconciles.
        tokio::spawn(async move { operation(&mut writer).await }).await?
    }

    pub(crate) async fn begin_display_capture(&self) {
        let mut writer = self.handles.writer.lock().await;
        writer.display_capture = None;
        if writer.lease.is_some()
            && let Some(records) = DisplayRecordWriter::new()
            && let Some(start) = writer.display_source().await
        {
            writer.display_capture = Some(DisplayCapture { start, records });
        }
    }

    pub(crate) async fn finish_display_capture(&self) -> Option<CommittedDisplay> {
        let mut writer = self.handles.writer.lock().await;
        let capture = writer.display_capture.take()?;
        if writer.revision == capture.start.revision {
            return None;
        }
        let lease = writer.lease.clone()?;
        let Response::Resumed { state } = writer
            .service
            .execute(Request::Resume { lease })
            .await
            .ok()?
        else {
            return None;
        };
        let end = writer.display_source().await?;
        Some(CommittedDisplay {
            start: capture.start,
            end,
            state,
            records: capture.records.finish()?,
        })
    }

    pub async fn append_journal_entry(
        &self,
        thread_id: ThreadId,
        turn_id: Option<TurnId>,
        entry: JournalEntry,
    ) -> Result<JournalRecord> {
        self.transaction(move |writer| {
            Box::pin(async move {
                writer
                    .commit(
                        StorageRecordAttribution {
                            execution_id: None,
                            thread_id: Some(thread_id),
                            turn_id,
                        },
                        entry,
                    )
                    .await
            })
        })
        .await
    }
    pub async fn append_execution_journal_entry(
        &self,
        attribution: ExecutionAttribution,
        entry: JournalEntry,
    ) -> Result<JournalRecord> {
        let (thread_id, turn_id) = match attribution.agent {
            Some(agent) => {
                ensure!(
                    agent.session_id == self.session_id(),
                    "execution attribution belongs to another session"
                );
                (Some(agent.thread_id), Some(agent.turn_id))
            }
            None => (None, None),
        };
        self.transaction(move |writer| {
            Box::pin(async move {
                writer
                    .commit(
                        StorageRecordAttribution {
                            execution_id: Some(attribution.execution_id),
                            thread_id,
                            turn_id,
                        },
                        entry,
                    )
                    .await
            })
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
        tool_results: Vec<proteus_contracts::contracts::WorkflowToolResultBinding>,
    ) -> Result<()> {
        self.transaction(move |writer| {
            Box::pin(async move {
                writer.acquire().await?;
                let previous_revision = writer.revision.history_revision;
                let entry = JournalEntry::HistoryMutated(HistoryMutated {
                    previous_revision,
                    new_revision: previous_revision
                        .checked_add(1)
                        .ok_or_else(|| anyhow!("history revision overflow"))?,
                    mutation,
                    messages,
                    compaction,
                    tool_results,
                });
                writer
                    .commit(
                        StorageRecordAttribution {
                            execution_id: None,
                            thread_id: Some(thread_id),
                            turn_id,
                        },
                        entry,
                    )
                    .await?;
                Ok(())
            })
        })
        .await
    }
    pub(crate) async fn checkpoint_history(
        &self,
        thread_id: ThreadId,
        turn_id: TurnId,
        messages: Vec<CanonicalMessage>,
        compaction: Option<HistoryCompactionReport>,
        tool_results: Vec<proteus_contracts::contracts::WorkflowToolResultBinding>,
    ) -> Result<()> {
        self.mutate_history(
            thread_id,
            Some(turn_id),
            messages,
            HistoryMutationKind::Checkpoint,
            compaction,
            tool_results,
        )
        .await
    }
    pub(crate) async fn history_revision(&self) -> Result<u64> {
        let writer = self.handles.writer.lock().await;
        if writer.lease.is_some() {
            return Ok(writer.revision.history_revision);
        }
        drop(writer);
        Ok(self.snapshot()?.descriptor().revision.history_revision)
    }
    pub(crate) async fn prepare_resume_state(
        &self,
    ) -> Result<super::snapshot::RemoteJournalResumeState> {
        self.transaction(|writer| {
            Box::pin(async move {
                match writer.read_state(|lease| Request::Resume { lease }).await? {
                    Response::Resumed { state } => {
                        writer.revision = state.conversation.revision;
                        Ok(state.into())
                    }
                    _ => bail!("storage returned an unexpected resume response"),
                }
            })
        })
        .await
    }
    pub(crate) async fn execution_state(
        &self,
    ) -> Result<(
        u64,
        Vec<proteus_contracts::contracts::WorkflowHistoryInterruption>,
    )> {
        self.transaction(|writer| {
            Box::pin(async move {
                match writer
                    .read_state(|lease| Request::ReadExecutionState { lease })
                    .await?
                {
                    Response::ExecutionState {
                        revision,
                        interrupted_turns,
                    } => {
                        writer.revision = revision;
                        Ok((revision.history_revision, interrupted_turns))
                    }
                    _ => bail!("storage returned an unexpected execution state"),
                }
            })
        })
        .await
    }
    pub(crate) async fn committed_history(
        &self,
        thread_id: ThreadId,
        turn_id: TurnId,
    ) -> Result<(Vec<CanonicalMessage>, usize)> {
        self.transaction(move |writer| {
            Box::pin(async move {
                match writer
                    .read_state(|lease| Request::ReadCommittedTurn {
                        lease,
                        thread_id,
                        turn_id,
                    })
                    .await?
                {
                    Response::CommittedTurn { state } => {
                        writer.revision = state.revision;
                        Ok((state.messages, state.changed_compactions))
                    }
                    _ => bail!("storage returned an unexpected committed history response"),
                }
            })
        })
        .await
    }
    pub(crate) async fn model_context_before_turn(
        &self,
        turn_id: TurnId,
    ) -> Result<Vec<proteus_contracts::contracts::ModelContextObservation>> {
        self.transaction(move |writer| {
            Box::pin(async move {
                match writer
                    .read_state(|lease| Request::ReadWriterModelContextBeforeTurn {
                        lease,
                        turn_id,
                    })
                    .await?
                {
                    Response::ModelContext { observations } => Ok(observations),
                    _ => bail!("storage returned an unexpected model context response"),
                }
            })
        })
        .await
    }

    pub(crate) async fn hibernate_writer(&self) {
        let _ = self
            .transaction(|writer| {
                Box::pin(async move {
                    if let Some(lease) = writer.lease.clone() {
                        writer
                            .service
                            .execute(Request::HibernateWriter { lease })
                            .await?;
                    }
                    Ok(())
                })
            })
            .await;
    }
    pub(crate) async fn put_blob(&self, bytes: Vec<u8>) -> Result<(String, PathBuf)> {
        use base64::Engine as _;
        let expected = proteus_contracts::contracts::StorageBlobRef {
            sha256: sha256_bytes(&bytes),
            bytes: u64::try_from(bytes.len())?,
        };
        let data_base64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        self.transaction(move |writer| {
            Box::pin(async move {
                let lease = writer.acquire().await?;
                match writer
                    .service
                    .execute(Request::PutBlob {
                        lease,
                        operation_id: uuid::Uuid::new_v4(),
                        data_base64,
                        expected,
                    })
                    .await?
                {
                    Response::Blob {
                        blob,
                        local_path: Some(path),
                    } => Ok((blob.sha256, path)),
                    _ => bail!("storage did not return a model-readable image location"),
                }
            })
        })
        .await
    }

    pub(crate) async fn write_metadata(&self, key: &str, value: serde_json::Value) -> Result<()> {
        let previous = self.read_metadata_document(key).await?;
        self.compare_exchange_metadata(key.to_owned(), previous.map(|doc| doc.etag), Some(value))
            .await?;
        Ok(())
    }
    pub(crate) async fn compare_exchange_metadata(
        &self,
        key: String,
        expected_etag: Option<String>,
        value: Option<serde_json::Value>,
    ) -> Result<Option<proteus_contracts::contracts::StorageMetadataDocument>> {
        self.transaction(move |writer| {
            Box::pin(async move {
                let lease = writer.acquire().await?;
                match writer
                    .service
                    .execute(Request::CompareExchangeMetadata {
                        lease,
                        operation_id: uuid::Uuid::new_v4(),
                        key,
                        expected_etag,
                        value,
                    })
                    .await?
                {
                    Response::Metadata { document } => Ok(document),
                    _ => bail!("storage returned an unexpected metadata write response"),
                }
            })
        })
        .await
    }
    pub(super) async fn delete(&self, owned: bool) -> Result<()> {
        self.transaction(move |writer| {
            Box::pin(async move {
                ensure!(
                    owned || writer.lease.is_none(),
                    "session has an active local writer"
                );
                let lease = writer.acquire().await?;
                match writer
                    .service
                    .execute(Request::Delete {
                        lease,
                        operation_id: uuid::Uuid::new_v4(),
                    })
                    .await?
                {
                    Response::Done => {
                        writer.lease = None;
                        writer.retired = true;
                        super::catalog::notify_update();
                        Ok(())
                    }
                    _ => bail!("storage returned an unexpected deletion response"),
                }
            })
        })
        .await
    }
}
