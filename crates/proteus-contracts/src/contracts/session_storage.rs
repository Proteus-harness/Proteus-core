//! Callback-free persistence boundary. Storage chooses encoding, indexes and
//! leases; callers own execution order and only advance after durable ACK.
use std::path::PathBuf;

use anyhow::{Result, ensure};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    contracts::{ModelContextObservation, WorkflowHistoryInterruption},
    domain::{ExecutionId, PartId, RecordId, SessionId, SessionUsageSnapshot, ThreadId, TurnId},
    model_standard::CanonicalMessage,
    storage::journal::{JournalEntry, JournalRecord},
};

pub const PROCESS_SESSION_STORAGE_CONTRACT_VERSION: &str = "v1";
pub const PROCESS_SESSION_STORAGE_EXECUTE_METHOD: &str = "execute";
pub const SESSION_STORAGE_MAX_PAGE_RECORDS: u32 = 1024;
pub const SESSION_STORAGE_PAGE_BYTE_BUDGET: usize = 4 * 1024 * 1024;
/// A page normally fits PAGE_BYTE_BUDGET. One larger canonical record may be
/// returned alone, within this explicit cap and the selected transport budget.
pub const SESSION_STORAGE_MAX_RECORD_BYTES: usize = 65 * 1024 * 1024;
pub const SESSION_STORAGE_MAX_METADATA_BYTES: usize = 1024 * 1024;
pub const SESSION_STORAGE_MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;
pub const SESSION_STORAGE_MAX_BLOB_BYTES: u64 = 64 * 1024 * 1024;

#[cfg(test)]
mod tests;
mod validation;

/// The key is interpreted only by the selected storage implementation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct SessionLocator {
    pub session_id: SessionId,
    pub key: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StorageRevision {
    pub session_seq: u64,
    pub history_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StorageSessionDescriptor {
    pub session: SessionLocator,
    pub parent_session_id: Option<SessionId>,
    pub workspace_path: PathBuf,
    /// Advisory location for display/export; it does not authorize direct reads.
    pub local_path: Option<PathBuf>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StorageSessionSummary {
    pub descriptor: StorageSessionDescriptor,
    pub message_count: usize,
    pub preview: Option<String>,
}

/// An immutable committed prefix; a restarted implementation may reject an
/// expired handle, but may never silently read a newer prefix under this id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StorageSnapshot {
    pub session: SessionLocator,
    pub snapshot_id: String,
    pub revision: StorageRevision,
    pub content_sha256: String,
}

/// Writer ownership is generation-fenced independently of history revision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StorageWriterLease {
    pub session: SessionLocator,
    pub token: Uuid,
    pub generation: Uuid,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StorageRecordAttribution {
    pub execution_id: Option<ExecutionId>,
    pub thread_id: Option<ThreadId>,
    pub turn_id: Option<TurnId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StorageCommit {
    pub lease: StorageWriterLease,
    /// Assigned before dispatch and stored atomically as the journal record id.
    /// Repetition with identical canonical content returns the original ACK.
    pub operation_id: RecordId,
    pub expected: StorageRevision,
    pub timestamp_ms: i64,
    pub attribution: StorageRecordAttribution,
    pub entry: JournalEntry,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StorageCommitAck {
    pub operation_id: RecordId,
    pub revision: StorageRevision,
    /// The canonical/redacted record actually committed, including its sequence.
    pub record: JournalRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StorageHistoryPoint {
    Current,
    Revision {
        history_revision: u64,
    },
    /// Snapshot immediately before the committed replacement introducing part.
    BeforeReplacement {
        part_id: PartId,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StorageConversation {
    pub messages: Vec<CanonicalMessage>,
    pub revision: StorageRevision,
    pub model_context: Vec<ModelContextObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StorageResumeState {
    pub conversation: StorageConversation,
    pub usage: SessionUsageSnapshot,
    pub last_history_turn: Option<TurnId>,
    pub interrupted_turns: Vec<WorkflowHistoryInterruption>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StorageCommittedTurn {
    pub messages: Vec<CanonicalMessage>,
    pub changed_compactions: usize,
    pub revision: StorageRevision,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StorageRecordPage {
    pub snapshot: StorageSnapshot,
    pub records: Vec<JournalRecord>,
    pub next_after_seq: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StorageMetadataDocument {
    pub etag: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StorageBlobRef {
    pub sha256: String,
    pub bytes: u64,
}

/// The same operations and permissions apply to every storage implementation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionStorageRequest {
    Create {
        config_root: PathBuf,
        workspace_path: PathBuf,
        session_id: SessionId,
        parent_session_id: Option<SessionId>,
    },
    Open {
        path: PathBuf,
    },
    Describe {
        session: SessionLocator,
    },
    List {
        config_root: PathBuf,
        workspace_path: Option<PathBuf>,
        audit: bool,
    },
    AcquireWriter {
        session: SessionLocator,
    },
    ReleaseWriter {
        lease: StorageWriterLease,
    },
    HibernateWriter {
        lease: StorageWriterLease,
    },
    Commit {
        commit: StorageCommit,
    },
    CommitStatus {
        session: SessionLocator,
        operation_id: RecordId,
    },
    OpenSnapshot {
        session: SessionLocator,
    },
    ReleaseSnapshot {
        snapshot: StorageSnapshot,
    },
    CompareSnapshots {
        snapshot: StorageSnapshot,
        prefix: StorageSnapshot,
    },
    ReadRecords {
        snapshot: StorageSnapshot,
        after_seq: u64,
        limit: u32,
    },
    ReadConversation {
        snapshot: StorageSnapshot,
        point: StorageHistoryPoint,
    },
    ReadUsage {
        snapshot: StorageSnapshot,
    },
    Resume {
        lease: StorageWriterLease,
    },
    ReadCommittedTurn {
        lease: StorageWriterLease,
        thread_id: ThreadId,
        turn_id: TurnId,
    },
    ReadModelContextBeforeTurn {
        snapshot: StorageSnapshot,
        turn_id: TurnId,
    },
    ReadMetadata {
        session: SessionLocator,
        key: String,
    },
    /// Disposable projections never acquire writer ownership or become journal
    /// facts. A source filter may require an exact fixed-prefix cache match.
    ReadCache {
        session: SessionLocator,
        key: String,
        source: Option<StorageSnapshot>,
    },
    /// Store only while the source is still current and the same session exists.
    /// A stale/deleted source returns Metadata { document: None }.
    WriteCache {
        session: SessionLocator,
        key: String,
        source: StorageSnapshot,
        value: serde_json::Value,
    },
    CompareExchangeMetadata {
        lease: StorageWriterLease,
        operation_id: Uuid,
        key: String,
        expected_etag: Option<String>,
        value: Option<serde_json::Value>,
    },
    PutBlob {
        lease: StorageWriterLease,
        operation_id: Uuid,
        data_base64: String,
        expected: StorageBlobRef,
    },
    ReadBlob {
        session: SessionLocator,
        blob: StorageBlobRef,
    },
    Delete {
        lease: StorageWriterLease,
        operation_id: Uuid,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionStorageResponse {
    Session {
        descriptor: StorageSessionDescriptor,
    },
    Sessions {
        sessions: Vec<StorageSessionSummary>,
    },
    Writer {
        lease: StorageWriterLease,
        revision: StorageRevision,
    },
    Committed {
        ack: StorageCommitAck,
    },
    CommitStatus {
        ack: Option<StorageCommitAck>,
    },
    Snapshot {
        snapshot: StorageSnapshot,
    },
    SnapshotPrefix {
        matches: bool,
    },
    Records {
        page: StorageRecordPage,
    },
    Conversation {
        conversation: StorageConversation,
    },
    Usage {
        usage: SessionUsageSnapshot,
    },
    Resumed {
        state: StorageResumeState,
    },
    CommittedTurn {
        state: StorageCommittedTurn,
    },
    ModelContext {
        observations: Vec<ModelContextObservation>,
    },
    Metadata {
        document: Option<StorageMetadataDocument>,
    },
    Blob {
        blob: StorageBlobRef,
    },
    BlobData {
        blob: StorageBlobRef,
        data_base64: String,
    },
    Done,
}

impl SessionStorageRequest {
    pub fn validate(&self) -> Result<()> {
        validation::validate_additional_request(self)?;
        if let Self::ReadRecords { limit, .. } = self {
            ensure!(
                (1..=SESSION_STORAGE_MAX_PAGE_RECORDS).contains(limit),
                "invalid storage record page limit"
            );
        }
        if let Self::ReadMetadata { key, .. }
        | Self::CompareExchangeMetadata { key, .. }
        | Self::ReadCache { key, .. }
        | Self::WriteCache { key, .. } = self
        {
            ensure!(
                !key.is_empty()
                    && key.len() <= 256
                    && !key.contains(['/', '\\'])
                    && key != "."
                    && key != "..",
                "invalid storage metadata key"
            );
        }
        if let Self::CompareExchangeMetadata {
            value: Some(value), ..
        } = self
        {
            ensure!(
                serde_json::to_vec(value)?.len() <= SESSION_STORAGE_MAX_METADATA_BYTES,
                "storage metadata is too large"
            );
        }
        if let Self::PutBlob {
            expected,
            data_base64,
            ..
        } = self
        {
            ensure!(
                expected.bytes <= SESSION_STORAGE_MAX_BLOB_BYTES,
                "storage blob is too large"
            );
            ensure!(
                data_base64.len() as u64 <= SESSION_STORAGE_MAX_BLOB_BYTES.div_ceil(3) * 4,
                "storage blob encoding is too large"
            );
        }
        Ok(())
    }
}

#[async_trait]
pub trait SessionStorage: Send + Sync {
    async fn execute(&self, request: SessionStorageRequest) -> Result<SessionStorageResponse>;
    fn execute_blocking(&self, request: SessionStorageRequest) -> Result<SessionStorageResponse>;
}
