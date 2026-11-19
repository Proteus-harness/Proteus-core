use super::*;
use crate::storage::journal::{JOURNAL_SCHEMA_VERSION, JournalEntry};

pub(super) fn validate_additional_request(request: &SessionStorageRequest) -> Result<()> {
    match request {
        SessionStorageRequest::CompareSnapshots { snapshot, prefix } => {
            validate_snapshot(snapshot)?;
            validate_snapshot(prefix)?;
            ensure!(
                snapshot.session == prefix.session,
                "storage snapshot comparison crosses sessions"
            );
        }
        SessionStorageRequest::ReadRecords {
            snapshot,
            after_seq,
            ..
        } => {
            validate_snapshot(snapshot)?;
            ensure!(
                *after_seq <= snapshot.revision.session_seq,
                "storage record cursor exceeds its snapshot"
            );
        }
        SessionStorageRequest::ReadConversation { snapshot, point } => {
            validate_snapshot(snapshot)?;
            if let StorageHistoryPoint::Revision { history_revision } = point {
                ensure!(
                    *history_revision <= snapshot.revision.history_revision,
                    "requested history revision exceeds its snapshot"
                );
            }
        }
        SessionStorageRequest::ReadCache {
            session,
            source: Some(source),
            ..
        } => {
            validate_snapshot(source)?;
            ensure!(
                *session == source.session,
                "storage cache source belongs to another session"
            );
        }
        SessionStorageRequest::WriteCache {
            session,
            source,
            value,
            ..
        } => {
            validate_snapshot(source)?;
            ensure!(
                *session == source.session,
                "storage cache source belongs to another session"
            );
            serde_json::to_writer(ByteCounter::new(SESSION_STORAGE_MAX_CACHE_BYTES), value)?;
        }
        SessionStorageRequest::ReadBlobByHash {
            sha256, max_bytes, ..
        } => {
            ensure!(
                *max_bytes <= SESSION_STORAGE_MAX_BLOB_BYTES,
                "storage blob read budget is too large"
            );
            ensure!(
                sha256.len() == 64 && sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "invalid storage blob digest"
            );
        }
        _ => {}
    }
    Ok(())
}

impl SessionStorageRequest {
    /// Validate the discriminant and binding of an untrusted process result.
    /// Canonical journal lifecycle validation remains a separate semantic fold.
    pub fn validate_response(&self, response: &SessionStorageResponse) -> Result<()> {
        match (self, response) {
            (
                Self::Create {
                    workspace_path,
                    session_id,
                    parent_session_id,
                    ..
                },
                SessionStorageResponse::Session { descriptor },
            ) => {
                validate_descriptor(descriptor)?;
                ensure!(
                    descriptor.session.session_id == *session_id
                        && descriptor.parent_session_id == *parent_session_id,
                    "storage returned a different created session"
                );
                ensure!(
                    descriptor.workspace_path == *workspace_path,
                    "storage returned a different workspace"
                );
            }
            (Self::Open { .. }, SessionStorageResponse::Session { descriptor }) => {
                validate_descriptor(descriptor)?
            }
            (Self::Describe { session }, SessionStorageResponse::Session { descriptor }) => {
                validate_descriptor(descriptor)?;
                ensure!(
                    descriptor.session == *session,
                    "storage descriptor belongs to another session"
                );
            }
            (Self::List { workspace_path, .. }, SessionStorageResponse::Sessions { sessions }) => {
                let mut identities = std::collections::HashSet::new();
                for session in sessions {
                    validate_descriptor(&session.descriptor)?;
                    ensure!(
                        identities.insert(&session.descriptor.session),
                        "storage catalog contains duplicate sessions"
                    );
                    if let Some(workspace) = workspace_path {
                        ensure!(
                            session.descriptor.workspace_path == *workspace,
                            "storage catalog returned another workspace"
                        );
                    }
                }
            }
            (
                Self::ListCandidates { workspace_path, .. },
                SessionStorageResponse::Candidates { sessions },
            ) => {
                let mut identities = std::collections::HashSet::new();
                for descriptor in sessions {
                    validate_descriptor(descriptor)?;
                    ensure!(
                        identities.insert(&descriptor.session),
                        "storage candidates contain duplicate sessions"
                    );
                    if let Some(workspace) = workspace_path {
                        ensure!(
                            descriptor.workspace_path == *workspace,
                            "storage candidate belongs to another workspace"
                        );
                    }
                }
            }
            (Self::AcquireWriter { session }, SessionStorageResponse::Writer { lease, .. }) => {
                ensure!(
                    lease.session == *session,
                    "writer lease belongs to another session"
                );
            }
            (Self::Commit { commit }, SessionStorageResponse::Committed { ack }) => {
                validate_ack(&commit.lease.session, commit.operation_id, ack)?;
                ensure!(
                    ack.record.session_seq
                        == commit
                            .expected
                            .session_seq
                            .checked_add(1)
                            .ok_or_else(|| anyhow::anyhow!("storage sequence overflow"))?,
                    "storage commit acknowledged the wrong sequence"
                );
                ensure!(
                    ack.record.execution_id == commit.attribution.execution_id
                        && ack.record.thread_id == commit.attribution.thread_id
                        && ack.record.turn_id == commit.attribution.turn_id,
                    "storage commit changed attribution"
                );
                ensure!(
                    ack.record.timestamp_ms == commit.timestamp_ms
                        && ack.record.entry.kind() == commit.entry.kind(),
                    "storage commit changed record identity"
                );
                let expected_history = match &commit.entry {
                    JournalEntry::HistoryMutated(mutation) => mutation.new_revision,
                    _ => commit.expected.history_revision,
                };
                ensure!(
                    ack.revision.history_revision == expected_history,
                    "storage commit acknowledged the wrong history revision"
                );
            }
            (
                Self::CommitStatus {
                    session,
                    operation_id,
                },
                SessionStorageResponse::CommitStatus { ack },
            ) => {
                if let Some(ack) = ack {
                    validate_ack(session, *operation_id, ack)?;
                }
            }
            (Self::OpenSnapshot { session }, SessionStorageResponse::Snapshot { snapshot }) => {
                validate_snapshot(snapshot)?;
                ensure!(
                    snapshot.session == *session,
                    "snapshot belongs to another session"
                );
            }
            (
                Self::ReadRecords {
                    snapshot,
                    after_seq,
                    limit,
                },
                SessionStorageResponse::Records { page },
            ) => {
                ensure!(
                    page.snapshot == *snapshot,
                    "storage changed the fixed record snapshot"
                );
                ensure!(
                    page.records.len() <= *limit as usize,
                    "storage exceeded record page bound"
                );
                let byte_budget = if page.records.len() == 1 {
                    SESSION_STORAGE_MAX_RECORD_BYTES
                } else {
                    SESSION_STORAGE_PAGE_BYTE_BUDGET
                };
                let mut counter = ByteCounter::new(byte_budget);
                for record in &page.records {
                    serde_json::to_writer(&mut counter, record)?;
                }
                let mut sequence = *after_seq;
                for record in &page.records {
                    sequence = sequence
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("storage sequence overflow"))?;
                    ensure!(
                        record.session_id == snapshot.session.session_id
                            && record.schema_version == JOURNAL_SCHEMA_VERSION
                            && record.session_seq == sequence
                            && sequence <= snapshot.revision.session_seq,
                        "storage record page escaped its committed prefix"
                    );
                }
                let next = (sequence < snapshot.revision.session_seq).then_some(sequence);
                ensure!(
                    page.next_after_seq == next && (next.is_none() || !page.records.is_empty()),
                    "storage record cursor does not advance"
                );
            }
            (Self::CompareSnapshots { .. }, SessionStorageResponse::SnapshotPrefix { .. }) => {}
            (
                Self::ReadConversation { snapshot, point },
                SessionStorageResponse::Conversation { conversation },
            ) => {
                ensure!(
                    conversation.revision.session_seq <= snapshot.revision.session_seq
                        && conversation.revision.history_revision
                            <= snapshot.revision.history_revision,
                    "conversation escaped its committed prefix"
                );
                match point {
                    StorageHistoryPoint::Current => ensure!(
                        conversation.revision == snapshot.revision,
                        "storage changed current conversation revision"
                    ),
                    StorageHistoryPoint::Revision { history_revision } => ensure!(
                        conversation.revision.history_revision == *history_revision,
                        "storage returned another history revision"
                    ),
                    StorageHistoryPoint::BeforeReplacement { .. } => {
                        ensure!(
                            conversation.revision.session_seq < snapshot.revision.session_seq
                                && conversation.revision.history_revision
                                    < snapshot.revision.history_revision,
                            "storage did not return a pre-replacement snapshot"
                        );
                    }
                }
            }
            (Self::ReadUsage { snapshot }, SessionStorageResponse::Usage { usage }) => {
                ensure!(
                    usage.session_id == snapshot.session.session_id
                        && usage.revision <= snapshot.revision.session_seq,
                    "usage escaped its committed prefix"
                );
            }
            (Self::Resume { lease }, SessionStorageResponse::Resumed { state }) => {
                ensure!(
                    state.usage.session_id == lease.session.session_id
                        && state.usage.revision <= state.conversation.revision.session_seq,
                    "resume facts belong to another session or prefix"
                );
            }
            (Self::ReadCommittedTurn { .. }, SessionStorageResponse::CommittedTurn { .. }) => {}
            (Self::ReadExecutionState { .. }, SessionStorageResponse::ExecutionState { .. }) => {}
            (
                Self::ReadModelContextBeforeTurn { .. }
                | Self::ReadWriterModelContextBeforeTurn { .. },
                SessionStorageResponse::ModelContext { .. },
            ) => {}
            (
                Self::ReadMetadata { .. } | Self::CompareExchangeMetadata { .. },
                SessionStorageResponse::Metadata { document },
            ) => {
                if let Self::CompareExchangeMetadata { value, .. } = self {
                    ensure!(
                        document.as_ref().map(|document| &document.value) == value.as_ref(),
                        "storage metadata ACK changed the committed value"
                    );
                }
                if let Some(document) = document {
                    ensure!(
                        !document.etag.is_empty(),
                        "storage metadata has no revision"
                    );
                    ensure!(
                        serde_json::to_vec(&document.value)?.len()
                            <= SESSION_STORAGE_MAX_METADATA_BYTES,
                        "storage metadata response is too large"
                    );
                }
            }
            (Self::ReadCache { .. }, SessionStorageResponse::Metadata { document }) => {
                if let Some(document) = document {
                    ensure!(!document.etag.is_empty(), "storage cache has no revision");
                    serde_json::to_writer(
                        ByteCounter::new(SESSION_STORAGE_MAX_CACHE_BYTES),
                        &document.value,
                    )?;
                }
            }
            (Self::WriteCache { value, .. }, SessionStorageResponse::CacheWritten { etag }) => {
                if let Some(etag) = etag {
                    ensure!(
                        *etag == storage_value_etag(value)?,
                        "storage cache ACK differs from the requested projection"
                    );
                }
            }
            (Self::PutBlob { expected, .. }, SessionStorageResponse::Blob { blob, .. }) => {
                ensure!(blob == expected, "storage returned another blob")
            }
            (
                Self::ReadBlobByHash {
                    sha256, max_bytes, ..
                },
                SessionStorageResponse::BlobData { blob, data_base64 },
            ) => {
                ensure!(
                    blob.sha256 == *sha256 && blob.bytes <= *max_bytes,
                    "storage blob escaped its requested digest or byte budget"
                );
                ensure!(
                    data_base64.len() as u64 <= max_bytes.div_ceil(3) * 4,
                    "storage blob encoding escaped its byte budget"
                );
            }
            (
                Self::ReadBlob { blob: expected, .. },
                SessionStorageResponse::BlobData { blob, data_base64 },
            ) => {
                ensure!(blob == expected, "storage returned another blob");
                ensure!(
                    data_base64.len() as u64 <= SESSION_STORAGE_MAX_BLOB_BYTES.div_ceil(3) * 4,
                    "storage blob response is too large"
                );
            }
            (
                Self::ReleaseWriter { .. }
                | Self::HibernateWriter { .. }
                | Self::ReleaseSnapshot { .. }
                | Self::Delete { .. },
                SessionStorageResponse::Done,
            ) => {}
            _ => anyhow::bail!("storage response does not match the requested operation"),
        }
        Ok(())
    }
}

fn validate_descriptor(descriptor: &StorageSessionDescriptor) -> Result<()> {
    ensure!(
        !descriptor.session.key.trim().is_empty(),
        "storage session locator is empty"
    );
    ensure!(
        descriptor.parent_session_id != Some(descriptor.session.session_id),
        "storage session is its own parent"
    );
    Ok(())
}

fn validate_snapshot(snapshot: &StorageSnapshot) -> Result<()> {
    ensure!(
        !snapshot.snapshot_id.trim().is_empty(),
        "storage snapshot id is empty"
    );
    ensure!(
        snapshot.content_sha256.len() == 64
            && snapshot
                .content_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "storage snapshot fingerprint is invalid"
    );
    Ok(())
}

fn validate_ack(
    session: &SessionLocator,
    operation_id: RecordId,
    ack: &StorageCommitAck,
) -> Result<()> {
    ensure!(
        ack.operation_id == operation_id
            && ack.record.record_id == operation_id
            && ack.record.session_id == session.session_id,
        "storage acknowledged another operation"
    );
    ensure!(
        ack.record.schema_version == JOURNAL_SCHEMA_VERSION
            && ack.record.session_seq == ack.revision.session_seq
            && ack.record.session_seq > 0,
        "storage ACK has an invalid committed record"
    );
    Ok(())
}

/// Count encoded payload bytes without allocating a second copy of record data.
struct ByteCounter {
    remaining: usize,
}

impl ByteCounter {
    fn new(limit: usize) -> Self {
        Self { remaining: limit }
    }
}

impl std::io::Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(std::io::Error::other(
                "storage record page exceeds its byte budget",
            ));
        }
        self.remaining -= bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
