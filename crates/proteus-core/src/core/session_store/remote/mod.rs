//! Session handles over the selected storage service. No physical journal codec.
use anyhow::{Result, anyhow, bail, ensure};
use proteus_contracts::{
    contracts::{
        SessionStorage, SessionStorageRequest as Request, SessionStorageResponse as Response,
        StorageHistoryPoint, StorageSessionDescriptor,
    },
    domain::{HistoryCompactionReport, SessionId, ThreadId, TurnId},
    model_standard::CanonicalMessage,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) mod catalog;
mod plain_history;
pub(crate) mod prepared;
pub(crate) mod snapshot;
pub(crate) mod view;
mod writer;
use writer::RemoteWriter;

pub type StorageService = Arc<dyn SessionStorage>;

#[derive(Clone)]
pub struct RemoteSessionStore {
    pub(crate) service: StorageService,
    pub(crate) descriptor: StorageSessionDescriptor,
    writer: Arc<tokio::sync::Mutex<RemoteWriter>>,
    created_root: Option<PathBuf>,
}

impl std::fmt::Debug for RemoteSessionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionStore")
            .field("session", &self.descriptor)
            .finish_non_exhaustive()
    }
}

impl RemoteSessionStore {
    pub fn new(
        service: StorageService,
        config_root: &Path,
        cwd: &Path,
        session_id: SessionId,
    ) -> Result<Self> {
        Self::new_with_parent(service, config_root, cwd, session_id, None)
    }

    pub fn new_with_parent(
        service: StorageService,
        config_root: &Path,
        cwd: &Path,
        session_id: SessionId,
        parent_session_id: Option<SessionId>,
    ) -> Result<Self> {
        ensure!(
            parent_session_id != Some(session_id),
            "session cannot be its own parent"
        );
        let root = crate::core::canonicalize_session_dir_path(config_root.to_owned())?;
        let response = service.execute_blocking(Request::Create {
            config_root: root.clone(),
            workspace_path: std::fs::canonicalize(cwd)?,
            session_id,
            parent_session_id,
        })?;
        let mut store = Self::from_descriptor(service, response)?;
        store.created_root = Some(root);
        Ok(store)
    }

    pub fn open(service: StorageService, path: PathBuf) -> Result<Self> {
        let response = service.execute_blocking(Request::Open { path })?;
        Self::from_descriptor(service, response)
    }

    fn from_descriptor(service: StorageService, response: Response) -> Result<Self> {
        let Response::Session { descriptor } = response else {
            bail!("storage returned an unexpected session response")
        };
        ensure!(
            descriptor.local_path.is_some(),
            "this client requires a session display locator"
        );
        Ok(Self {
            writer: writer::shared_writer(&service, &descriptor.session),
            service,
            descriptor,
            created_root: None,
        })
    }

    pub fn session_dir(&self) -> &Path {
        self.descriptor
            .local_path
            .as_deref()
            .expect("validated display locator")
    }
    pub fn session_id(&self) -> SessionId {
        self.descriptor.session.session_id
    }
    pub fn parent_session_id(&self) -> Option<SessionId> {
        self.descriptor.parent_session_id
    }
    pub fn workspace_path(&self) -> Result<PathBuf> {
        Ok(self.descriptor.workspace_path.clone())
    }
    /// Advisory location retained in diagnostics. Reads always use the service.
    pub fn journal_path(&self) -> PathBuf {
        self.descriptor
            .record_location
            .clone()
            .unwrap_or_else(|| self.session_dir().to_owned())
    }

    pub fn storage_service(&self) -> StorageService {
        self.service.clone()
    }

    pub fn journal_snapshot(&self) -> Result<proteus_contracts::contracts::StorageSnapshot> {
        Ok(self.snapshot()?.descriptor().clone())
    }

    pub fn read_history_at(
        &self,
        point: StorageHistoryPoint,
    ) -> Result<proteus_contracts::contracts::StorageConversation> {
        self.snapshot()?.conversation(point)
    }

    pub async fn usage_snapshot(&self) -> Result<proteus_contracts::domain::SessionUsageSnapshot> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(usage) = store.fresh_read_usage() {
                return Ok(usage);
            }
            let snapshot = store.snapshot()?;
            match store.request(Request::ReadUsage {
                snapshot: snapshot.descriptor().clone(),
            })? {
                Response::Usage { usage } => Ok(usage),
                _ => bail!("storage returned an unexpected usage response"),
            }
        })
        .await?
    }

    pub(crate) fn read_blob_by_hash(&self, sha256: String, max_bytes: u64) -> Result<Vec<u8>> {
        use base64::Engine as _;
        match self.request(Request::ReadBlobByHash {
            session: self.descriptor.session.clone(),
            sha256,
            max_bytes,
        })? {
            Response::BlobData { blob, data_base64 } => {
                let bytes = base64::engine::general_purpose::STANDARD.decode(data_base64)?;
                ensure!(
                    u64::try_from(bytes.len())? == blob.bytes && blob.bytes <= max_bytes,
                    "storage returned an invalid blob length"
                );
                ensure!(
                    blob.sha256 == sha256_bytes(&bytes),
                    "storage returned a different blob"
                );
                Ok(bytes)
            }
            _ => bail!("storage returned an unexpected blob response"),
        }
    }

    pub(crate) fn request(&self, request: Request) -> Result<Response> {
        self.service.execute_blocking(request)
    }
    pub(crate) fn snapshot(&self) -> Result<snapshot::RemoteJournalSnapshot> {
        snapshot::RemoteJournalSnapshot::open(self.clone())
    }

    pub fn conversation_snapshot(
        &self,
    ) -> Result<proteus_contracts::contracts::ConversationSnapshot> {
        let snapshot = self.snapshot()?;
        let conversation = snapshot.conversation(StorageHistoryPoint::Current)?;
        Ok(proteus_contracts::contracts::ConversationSnapshot {
            revision: conversation.revision.history_revision,
            messages: conversation.messages,
            model_context: conversation.model_context,
        })
    }

    pub fn load_messages(&self) -> Result<Vec<CanonicalMessage>> {
        Ok(self.conversation_snapshot()?.messages)
    }
    pub fn load_records(&self) -> Result<Vec<proteus_contracts::storage::journal::JournalRecord>> {
        let mut snapshot = self.snapshot()?;
        let mut records = Vec::new();
        snapshot.scan(|record| {
            records.push(record);
            Ok(())
        })?;
        Ok(records)
    }
    pub fn load_projection(&self) -> Result<proteus_session_semantics::JournalProjection> {
        Ok(self.load_projection_with_snapshot()?.0)
    }

    pub fn load_projection_with_snapshot(
        &self,
    ) -> Result<(
        proteus_session_semantics::JournalProjection,
        proteus_contracts::contracts::StorageSnapshot,
    )> {
        let mut snapshot = self.snapshot()?;
        let mut records = Vec::new();
        snapshot.scan(|record| {
            records.push(record);
            Ok(())
        })?;
        let projection =
            proteus_session_semantics::JournalProjection::build(self.session_id(), records)?;
        Ok((projection, snapshot.descriptor().clone()))
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
            proteus_contracts::storage::journal::HistoryMutationKind::Append,
            None,
            vec![],
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
            proteus_contracts::storage::journal::HistoryMutationKind::Replace,
            compaction,
            vec![],
        )
        .await
    }
    pub async fn clear_history(&self, thread_id: ThreadId) -> Result<()> {
        self.replace_history(thread_id, None, &[], None).await
    }

    pub(crate) async fn read_metadata_document(
        &self,
        key: &str,
    ) -> Result<Option<proteus_contracts::contracts::StorageMetadataDocument>> {
        let store = self.clone();
        let key = key.to_owned();
        tokio::task::spawn_blocking(move || store.read_metadata(&key)).await?
    }

    pub(crate) fn read_metadata(
        &self,
        key: &str,
    ) -> Result<Option<proteus_contracts::contracts::StorageMetadataDocument>> {
        match self.request(Request::ReadMetadata {
            session: self.descriptor.session.clone(),
            key: key.to_owned(),
        })? {
            Response::Metadata { document } => Ok(document),
            _ => Err(anyhow!("storage returned an unexpected metadata response")),
        }
    }
}

fn sha256_bytes(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
