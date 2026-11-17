use super::*;
use crate::{
    domain::{new_record_id, new_session_id, new_thread_id},
    model_standard::MessageRole,
    storage::journal::{HistoryMutated, HistoryMutationKind, JOURNAL_SCHEMA_VERSION},
};

fn session() -> SessionLocator {
    SessionLocator {
        session_id: new_session_id(),
        key: "opaque-storage-key".into(),
    }
}

fn snapshot() -> StorageSnapshot {
    StorageSnapshot {
        session: session(),
        snapshot_id: "immutable-prefix".into(),
        revision: StorageRevision {
            session_seq: 2,
            history_revision: 1,
        },
        content_sha256: "a".repeat(64),
    }
}

fn commit_fixture() -> (StorageCommit, StorageCommitAck) {
    let operation_id = new_record_id();
    let session = session();
    let entry = JournalEntry::HistoryMutated(HistoryMutated {
        previous_revision: 0,
        new_revision: 1,
        mutation: HistoryMutationKind::Append,
        messages: vec![CanonicalMessage::text(MessageRole::User, "canonical user")],
        compaction: None,
        tool_results: vec![],
    });
    let attribution = StorageRecordAttribution {
        execution_id: None,
        thread_id: Some(new_thread_id()),
        turn_id: None,
    };
    let commit = StorageCommit {
        lease: StorageWriterLease {
            session: session.clone(),
            token: Uuid::new_v4(),
            generation: Uuid::new_v4(),
        },
        operation_id,
        expected: StorageRevision::default(),
        timestamp_ms: 42,
        attribution,
        entry: entry.clone(),
    };
    let ack = StorageCommitAck {
        operation_id,
        revision: StorageRevision {
            session_seq: 1,
            history_revision: 1,
        },
        record: JournalRecord {
            schema_version: JOURNAL_SCHEMA_VERSION,
            record_id: operation_id,
            session_seq: 1,
            timestamp_ms: 42,
            session_id: session.session_id,
            execution_id: None,
            thread_id: attribution.thread_id,
            turn_id: None,
            entry,
        },
    };
    (commit, ack)
}

#[test]
fn durable_ack_is_bound_to_the_requested_record_and_both_revisions() {
    let (commit, ack) = commit_fixture();
    let request = SessionStorageRequest::Commit { commit };
    request
        .validate_response(&SessionStorageResponse::Committed { ack: ack.clone() })
        .unwrap();
    for variant in 0..5 {
        let mut wrong = ack.clone();
        match variant {
            0 => wrong.operation_id = new_record_id(),
            1 => wrong.record.session_id = new_session_id(),
            2 => wrong.record.session_seq += 1,
            3 => wrong.revision.history_revision += 1,
            _ => wrong.record.thread_id = Some(new_thread_id()),
        }
        assert!(
            request
                .validate_response(&SessionStorageResponse::Committed { ack: wrong })
                .is_err()
        );
    }
    assert!(
        request
            .validate_response(&SessionStorageResponse::Done)
            .is_err()
    );
}

#[test]
fn fixed_prefix_pages_reject_nonadvancing_cursors_and_another_snapshot() {
    let snapshot = snapshot();
    let request = SessionStorageRequest::ReadRecords {
        snapshot: snapshot.clone(),
        after_seq: 1,
        limit: 1,
    };
    let (_, mut ack) = commit_fixture();
    ack.record.session_id = snapshot.session.session_id;
    ack.record.session_seq = 2;
    let page = StorageRecordPage {
        snapshot: snapshot.clone(),
        records: vec![ack.record],
        next_after_seq: None,
    };
    request
        .validate_response(&SessionStorageResponse::Records { page: page.clone() })
        .unwrap();
    let mut wrong = page.clone();
    wrong.snapshot.snapshot_id.push('!');
    assert!(
        request
            .validate_response(&SessionStorageResponse::Records { page: wrong })
            .is_err()
    );
    let empty = StorageRecordPage {
        snapshot,
        records: vec![],
        next_after_seq: Some(1),
    };
    assert!(
        request
            .validate_response(&SessionStorageResponse::Records { page: empty })
            .is_err()
    );
}

#[test]
fn storage_wire_is_strict_and_requests_are_bounded() {
    let (commit, _) = commit_fixture();
    let value = serde_json::to_value(SessionStorageRequest::Commit { commit }).unwrap();
    let mut unknown = value.clone();
    unknown["commit"]["entry"]["payload"]["legacy"] = serde_json::json!(true);
    assert!(serde_json::from_value::<SessionStorageRequest>(unknown).is_err());
    serde_json::from_value::<SessionStorageRequest>(value).unwrap();
    assert!(
        SessionStorageRequest::ReadRecords {
            snapshot: snapshot(),
            after_seq: 0,
            limit: 0
        }
        .validate()
        .is_err()
    );
    assert!(
        SessionStorageRequest::ReadRecords {
            snapshot: snapshot(),
            after_seq: 0,
            limit: SESSION_STORAGE_MAX_PAGE_RECORDS + 1
        }
        .validate()
        .is_err()
    );
    assert!(
        SessionStorageRequest::ReadMetadata {
            session: session(),
            key: "../another-session".into()
        }
        .validate()
        .is_err()
    );
    let (commit, _) = commit_fixture();
    let metadata = SessionStorageRequest::CompareExchangeMetadata {
        lease: commit.lease,
        operation_id: Uuid::new_v4(),
        key: "selection".into(),
        expected_etag: None,
        value: Some(serde_json::json!({"model":"selected"})),
    };
    assert!(
        metadata
            .validate_response(&SessionStorageResponse::Metadata { document: None })
            .is_err()
    );
}

#[test]
fn journal_wire_roundtrip_preserves_existing_canonical_payload_and_schema() {
    let (_, ack) = commit_fixture();
    let encoded = serde_json::to_value(&ack.record).unwrap();
    assert_eq!(encoded["schema_version"], 21);
    let decoded: JournalRecord = serde_json::from_value(encoded).unwrap();
    let payload = decoded.entry.payload_value().unwrap();
    let from_payload = JournalEntry::from_kind_and_bytes(
        decoded.entry.kind(),
        &serde_json::to_vec(&payload).unwrap(),
    )
    .unwrap();
    assert_eq!(from_payload, ack.record.entry);
    assert_eq!(decoded, ack.record);
}

#[test]
fn prefix_comparison_is_session_bound_and_cache_read_can_request_stale_data() {
    let current = snapshot();
    let mut prefix = current.clone();
    prefix.snapshot_id = "expired-handle-with-known-fingerprint".into();
    prefix.revision.session_seq = 1;
    let comparison = SessionStorageRequest::CompareSnapshots {
        snapshot: current.clone(),
        prefix: prefix.clone(),
    };
    comparison.validate().unwrap();
    comparison
        .validate_response(&SessionStorageResponse::SnapshotPrefix { matches: true })
        .unwrap();
    prefix.session.session_id = new_session_id();
    assert!(
        SessionStorageRequest::CompareSnapshots {
            snapshot: current.clone(),
            prefix
        }
        .validate()
        .is_err()
    );
    let cache = SessionStorageRequest::ReadCache {
        session: current.session.clone(),
        key: "view-head".into(),
        source: None,
    };
    cache.validate().unwrap();
    cache
        .validate_response(&SessionStorageResponse::Metadata {
            document: Some(StorageMetadataDocument {
                etag: "old-generation".into(),
                value: serde_json::json!({"source": current, "stale": true}),
            }),
        })
        .unwrap();
    assert!(
        SessionStorageRequest::WriteCache {
            session: session(),
            key: "view-head".into(),
            source: snapshot(),
            value: serde_json::Value::Null
        }
        .validate()
        .is_err()
    );
}

#[test]
fn record_page_byte_budget_is_bounded_without_rejecting_one_large_record() {
    let (_, mut ack) = commit_fixture();
    let JournalEntry::HistoryMutated(mutation) = &mut ack.record.entry else {
        unreachable!()
    };
    mutation.messages = vec![CanonicalMessage::text(
        MessageRole::User,
        "x".repeat(3 * 1024 * 1024),
    )];
    let mut snapshot = snapshot();
    snapshot.session.session_id = ack.record.session_id;
    snapshot.revision.session_seq = 1;
    let request = SessionStorageRequest::ReadRecords {
        snapshot: snapshot.clone(),
        after_seq: 0,
        limit: 2,
    };
    let page = StorageRecordPage {
        snapshot: snapshot.clone(),
        records: vec![ack.record.clone()],
        next_after_seq: None,
    };
    request
        .validate_response(&SessionStorageResponse::Records { page })
        .unwrap();
    snapshot.revision.session_seq = 2;
    let request = SessionStorageRequest::ReadRecords {
        snapshot: snapshot.clone(),
        after_seq: 0,
        limit: 2,
    };
    let mut second = ack.record.clone();
    second.record_id = new_record_id();
    second.session_seq = 2;
    let page = StorageRecordPage {
        snapshot,
        records: vec![ack.record, second],
        next_after_seq: None,
    };
    assert!(
        request
            .validate_response(&SessionStorageResponse::Records { page })
            .is_err()
    );
}

#[test]
fn cache_write_receipt_binds_content_without_echoing_the_projection() {
    let source = snapshot();
    let value = serde_json::json!({"z":2,"a":[1,true]});
    let etag = storage_value_etag(&value).unwrap();
    assert_eq!(
        etag,
        "cab2365c93b4de12e8e2b0ec8b7a41a801a294b46c571adfdf87eed1d2342b2a"
    );
    let request = SessionStorageRequest::WriteCache {
        session: source.session.clone(),
        key: "header".into(),
        source,
        value,
    };
    request
        .validate_response(&SessionStorageResponse::CacheWritten { etag: Some(etag) })
        .unwrap();
    request
        .validate_response(&SessionStorageResponse::CacheWritten { etag: None })
        .unwrap();
    assert!(
        request
            .validate_response(&SessionStorageResponse::CacheWritten {
                etag: Some("wrong-value".into())
            })
            .is_err()
    );
    assert!(
        request
            .validate_response(&SessionStorageResponse::Metadata { document: None })
            .is_err()
    );
}

#[test]
fn writer_model_context_request_roundtrips_and_accepts_only_model_context_response() {
    let request = SessionStorageRequest::ReadWriterModelContextBeforeTurn {
        lease: StorageWriterLease {
            session: session(),
            token: Uuid::new_v4(),
            generation: Uuid::new_v4(),
        },
        turn_id: crate::domain::new_turn_id(),
    };
    request.validate().unwrap();
    let encoded = serde_json::to_value(&request).unwrap();
    assert_eq!(
        encoded["operation"],
        "read_writer_model_context_before_turn"
    );
    assert_eq!(
        serde_json::from_value::<SessionStorageRequest>(encoded).unwrap(),
        request
    );
    request
        .validate_response(&SessionStorageResponse::ModelContext {
            observations: vec![],
        })
        .unwrap();
    assert!(
        request
            .validate_response(&SessionStorageResponse::Done)
            .is_err()
    );
}
