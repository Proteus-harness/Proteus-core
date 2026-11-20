use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::{Result, bail};
use async_trait::async_trait;
use proteus_contracts::{
    contracts::{
        SessionLocator, SessionStorage, SessionStorageRequest as Request,
        SessionStorageResponse as Response, StorageMetadataDocument, StorageRevision,
        StorageSessionDescriptor, StorageSnapshot,
    },
    domain::{new_record_id, new_session_id, new_thread_id},
    model_standard::{CanonicalMessage, MessageRole},
    storage::journal::{
        HistoryMutated, HistoryMutationKind, JOURNAL_SCHEMA_VERSION, JournalEntry, JournalRecord,
    },
};
use serde_json::Value;

use super::super::{AppTranscriptMessage, RemoteSessionStore, SessionTranscriptAppend};
use crate::core::session_store::remote::snapshot::{RemoteJournalCursor, RemoteJournalSnapshot};

pub(super) struct State {
    pub(super) descriptor: StorageSessionDescriptor,
    pub(super) records: Vec<JournalRecord>,
    pub(super) cache: HashMap<String, Value>,
    pub(super) writes: Vec<String>,
    pub(super) cache_reads: Vec<String>,
    pub(super) record_reads: usize,
    pub(super) fail_head: bool,
    pub(super) append_on_cache_read: Option<String>,
    pub(super) handles: u64,
}

pub(super) struct Backend(pub(super) Mutex<State>);

impl Backend {
    pub(super) fn store() -> (Arc<Self>, RemoteSessionStore) {
        let session_id = new_session_id();
        let descriptor = StorageSessionDescriptor {
            session: SessionLocator {
                session_id,
                key: "cache-fixture".into(),
            },
            parent_session_id: None,
            workspace_path: PathBuf::from("/workspace"),
            local_path: Some(PathBuf::from("/session-cache-fixture")),
            record_location: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        let backend = Arc::new(Self(Mutex::new(State {
            descriptor,
            records: vec![],
            cache: HashMap::new(),
            writes: vec![],
            cache_reads: vec![],
            record_reads: 0,
            handles: 0,
            fail_head: false,
            append_on_cache_read: None,
        })));
        let store =
            RemoteSessionStore::open(backend.clone(), PathBuf::from("/session-cache-fixture"))
                .unwrap();
        (backend, store)
    }

    pub(super) fn append(&self, count: usize) {
        let mut state = self.0.lock().unwrap();
        let previous = state.records.len() as u64;
        let messages = (0..count)
            .map(|index| {
                CanonicalMessage::text(MessageRole::User, format!("message-{previous}-{index}"))
            })
            .collect();
        let record = JournalRecord {
            schema_version: JOURNAL_SCHEMA_VERSION,
            record_id: new_record_id(),
            session_seq: previous + 1,
            timestamp_ms: previous as i64 + 1,
            session_id: state.descriptor.session.session_id,
            execution_id: None,
            thread_id: Some(new_thread_id()),
            turn_id: None,
            entry: JournalEntry::HistoryMutated(HistoryMutated {
                previous_revision: previous,
                new_revision: previous + 1,
                mutation: HistoryMutationKind::Append,
                messages,
                compaction: None,
                tool_results: vec![],
            }),
        };
        state.records.push(record);
        state.descriptor.updated_at_ms += 1;
    }
}

fn snapshot(state: &mut State) -> StorageSnapshot {
    state.handles += 1;
    StorageSnapshot {
        session: state.descriptor.session.clone(),
        snapshot_id: state.handles.to_string(),
        revision: StorageRevision {
            session_seq: state.records.len() as u64,
            history_revision: state.records.len() as u64,
        },
        content_sha256: format!("{:064x}", state.records.len()),
    }
}

fn current(state: &State, source: &StorageSnapshot) -> bool {
    source.session == state.descriptor.session
        && source.revision.session_seq == state.records.len() as u64
        && source.revision.history_revision == state.records.len() as u64
        && source.content_sha256 == format!("{:064x}", state.records.len())
}

#[async_trait]
impl SessionStorage for Backend {
    async fn execute(&self, request: Request) -> Result<Response> {
        self.execute_blocking(request)
    }

    fn execute_blocking(&self, request: Request) -> Result<Response> {
        let mut state = self.0.lock().unwrap();
        match request {
            Request::Create { .. } | Request::Open { .. } | Request::Describe { .. } => {
                Ok(Response::Session {
                    descriptor: state.descriptor.clone(),
                })
            }
            Request::OpenSnapshot { .. } => Ok(Response::Snapshot {
                snapshot: snapshot(&mut state),
            }),
            Request::ReleaseSnapshot { .. } => Ok(Response::Done),
            Request::CompareSnapshots { snapshot, prefix } => Ok(Response::SnapshotPrefix {
                matches: snapshot.session == prefix.session
                    && snapshot.revision.session_seq >= prefix.revision.session_seq
                    && prefix.content_sha256 == format!("{:064x}", prefix.revision.session_seq),
            }),
            Request::ReadRecords {
                snapshot,
                after_seq,
                limit,
            } => {
                state.record_reads += 1;
                let records: Vec<_> = state
                    .records
                    .iter()
                    .filter(|record| {
                        record.session_seq > after_seq
                            && record.session_seq <= snapshot.revision.session_seq
                    })
                    .take(limit as usize)
                    .cloned()
                    .collect();
                let next_after_seq = records.last().and_then(|record| {
                    (record.session_seq < snapshot.revision.session_seq)
                        .then_some(record.session_seq)
                });
                Ok(Response::Records {
                    page: proteus_contracts::contracts::StorageRecordPage {
                        snapshot,
                        records,
                        next_after_seq,
                    },
                })
            }
            Request::ReadCache { key, source, .. } => {
                state.cache_reads.push(key.clone());
                let value = if source.as_ref().is_none_or(|source| current(&state, source)) {
                    state.cache.get(&key).cloned()
                } else {
                    None
                };
                if state.append_on_cache_read.as_ref() == Some(&key) {
                    state.append_on_cache_read = None;
                    drop(state);
                    self.append(1);
                }
                Ok(Response::Metadata {
                    document: value.map(|value| StorageMetadataDocument {
                        etag: "test".into(),
                        value,
                    }),
                })
            }
            Request::WriteCache {
                key, source, value, ..
            } => {
                if !current(&state, &source) || (state.fail_head && key == "display-view-v1-head") {
                    return Ok(Response::CacheWritten { etag: None });
                }
                state.writes.push(key.clone());
                state.cache.insert(key, value.clone());
                Ok(Response::CacheWritten {
                    etag: Some(proteus_contracts::contracts::storage_value_etag(&value)?),
                })
            }
            _ => bail!("unexpected cache-test request: {request:?}"),
        }
    }
}

pub(super) fn row(message: &CanonicalMessage, timestamp_ms: i64) -> AppTranscriptMessage {
    AppTranscriptMessage {
        message_id: Some(message.id),
        timestamp_ms: Some(timestamp_ms),
        phase: None,
        role: "user".into(),
        text: message
            .parts
            .iter()
            .filter_map(|part| match &part.payload {
                proteus_contracts::model_standard::ContentPart::Text { text } => {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        images: vec![],
        tool: None,
        subagent: None,
        streaming: false,
    }
}

pub(super) fn full(source: &mut RemoteJournalSnapshot) -> Result<Vec<AppTranscriptMessage>> {
    let mut rows = vec![];
    source.scan(|record| {
        if let JournalEntry::HistoryMutated(mutation) = record.entry {
            rows.extend(
                mutation
                    .messages
                    .iter()
                    .map(|message| row(message, record.timestamp_ms)),
            );
        }
        Ok(())
    })?;
    Ok(rows)
}

pub(super) fn append(
    source: &mut RemoteJournalSnapshot,
    previous: &RemoteJournalCursor,
    baseline: &[[u8; 32]],
    final_len: usize,
) -> Result<Option<SessionTranscriptAppend>> {
    let mut transcript = vec![];
    let mut added = 0usize;
    let matched = source.scan_selected_after(
        previous,
        |_| true,
        |record| {
            if let JournalEntry::HistoryMutated(mutation) = record.entry {
                added += mutation.messages.len();
                transcript.extend(
                    mutation
                        .messages
                        .iter()
                        .map(|message| row(message, record.timestamp_ms)),
                );
            }
            Ok(())
        },
    )?;
    Ok(
        (matched && baseline.len() + added == final_len).then_some(SessionTranscriptAppend {
            transcript,
            conversation_appended: added,
        }),
    )
}
