mod support;

use super::*;
use support::{Backend, append, full};

#[test]
fn prepared_view_never_mixes_an_older_summary_with_a_newer_transcript() {
    use super::super::prepared::PreparedSession;

    for retain_old_page in [false, true] {
        let (backend, store) = Backend::store();
        backend.append(51);
        let prepared =
            PreparedSession::from_dir(backend.clone(), store.session_dir().to_owned()).unwrap();
        let old_revision = RemoteCacheSource::read(&store).unwrap().revision().unwrap();
        if retain_old_page {
            store.project_read_model(full).unwrap();
        }
        // A second reader publishes a newer head before startup renders its fixed snapshot.
        backend.append(1);
        store.project_read_model(full).unwrap();

        let opens = backend.0.lock().unwrap().handles;
        let (_, view, _) = prepared.into_display_view(full).unwrap();
        assert_eq!(view.revision, 1);
        assert_eq!(view.summary.message_count, 51);
        assert_eq!(view.transcript.last().unwrap().text, "message-0-50");
        assert_eq!(backend.0.lock().unwrap().handles, opens);
        if retain_old_page {
            let cursor = view.cursor.expect("retained immutable page");
            assert_eq!(cursor.revision, old_revision);
            assert_eq!(cursor.before, 1);
            assert_eq!(view.transcript.len(), 50);
        } else {
            assert!(view.cursor.is_none());
            assert_eq!(view.transcript.len(), 51);
        }
    }
}

#[test]
fn cold_display_reuses_the_validated_records_without_another_storage_traversal() {
    let (backend, store) = Backend::store();
    for _ in 0..80 {
        backend.append(1);
    }
    let model = store
        .project_read_model(|source| {
            crate::app_server::journal_transcript_from_source(source, None)
        })
        .unwrap();
    assert_eq!(model.transcript.len(), 80);
    assert_eq!(backend.0.lock().unwrap().record_reads, 2);
}

#[test]
fn acknowledged_turn_updates_saved_pages_without_reading_the_journal() {
    use crate::core::{JournalEntry, session_store::remote::snapshot::DisplayRecordWriter};
    use proteus_contracts::contracts::{StorageConversation, StorageResumeState};

    let (backend, store) = Backend::store();
    backend.append(75);
    let baseline = store.project_read_model(full).unwrap();
    let start = RemoteCacheSource::read(&store).unwrap().snapshot;
    let mut records = DisplayRecordWriter::new().unwrap();
    let mut messages = baseline.conversation.messages;
    backend.append(1);
    {
        let backend = backend.0.lock().unwrap();
        let record = backend.records.last().unwrap();
        records.push(record);
        let JournalEntry::HistoryMutated(mutation) = &record.entry else {
            panic!("expected a committed append");
        };
        messages.extend(mutation.messages.clone());
    }
    let end = RemoteCacheSource::read(&store).unwrap().snapshot;
    let reads = backend.0.lock().unwrap().record_reads;
    let view = store
        .publish_committed_view(
            CommittedDisplay {
                start,
                end: end.clone(),
                state: StorageResumeState {
                    conversation: StorageConversation {
                        messages,
                        revision: end.revision,
                        model_context: vec![],
                    },
                    usage: baseline.usage,
                    last_history_turn: None,
                    interrupted_turns: vec![],
                },
                records: records.finish().unwrap(),
            },
            crate::app_server::journal_transcript_append_committed,
        )
        .expect("acknowledged independent append must publish from the committed records");
    assert_eq!(view.cursor.unwrap().base.unwrap().common_prefix, 75);
    let saved = store.fresh_read_model().unwrap();
    assert_eq!(backend.0.lock().unwrap().record_reads, reads);
    let replayed = SessionReadModel::from_validated(
        &store,
        RemoteValidatedRead::open(store.clone()).unwrap(),
        full,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(saved.transcript).unwrap(),
        serde_json::to_value(replayed.transcript).unwrap()
    );
    assert_eq!(saved.conversation, replayed.conversation);
}

#[test]
fn remote_display_append_reuses_prefix_pages_and_matches_full_projection() {
    let (backend, store) = Backend::store();
    backend.append(75);
    let first = store.project_read_view(full, append).unwrap();
    let first_cursor = first.cursor.unwrap();
    let header = cache::header(&store).unwrap();
    let retained = format!("display-data-{}", header.conversation.pages[0].hash);
    backend.0.lock().unwrap().writes.clear();
    backend.append(1);
    let view = store
        .project_read_view(
            |_| panic!("an independent append must use its suffix"),
            append,
        )
        .unwrap();
    let cursor = view.cursor.unwrap();
    assert_eq!(view.transcript.len(), 50);
    assert_eq!(view.summary.message_count, 76);
    assert_eq!(cursor.base.as_ref().unwrap().common_prefix, 75);
    assert_eq!(cursor.base.unwrap().revision, first_cursor.revision);
    assert!(!backend.0.lock().unwrap().writes.contains(&retained));
    let current = store.fresh_read_model().unwrap();
    let expected = SessionReadModel::from_validated(
        &store,
        RemoteValidatedRead::open(store.clone()).unwrap(),
        full,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(current).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

#[test]
fn stale_remote_view_is_display_only_and_old_revision_remains_coherent() {
    let (backend, store) = Backend::store();
    backend.append(75);
    let old = store.project_read_view(full, append).unwrap();
    let revision = old.cursor.unwrap().revision;
    backend.append(1);
    assert!(store.fresh_read_model().is_none());
    assert!(store.saved_transcript_page(None, None).is_none());
    assert_eq!(store.saved_display_page().unwrap().total, 75);
    let handles = backend.0.lock().unwrap().handles;
    let prepared = super::super::prepared::PreparedSession::from_dir(
        backend.clone(),
        store.session_dir().to_owned(),
    )
    .unwrap();
    let (_, provisional, cached_source) = prepared
        .into_display_view(|_| panic!("startup should use a bounded saved display page"))
        .unwrap();
    assert!(cached_source.is_some());
    assert_eq!(provisional.transcript.len(), 50);
    assert_eq!(backend.0.lock().unwrap().handles, handles);
    store.project_read_view(full, append).unwrap();
    assert_eq!(store.saved_transcript(&revision).unwrap().len(), 75);
}

#[test]
fn missing_requested_page_rebuilds_and_failed_publication_keeps_verified_result() {
    let (backend, store) = Backend::store();
    backend.append(75);
    store.project_read_view(full, append).unwrap();
    let header = cache::header(&store).unwrap();
    backend.0.lock().unwrap().cache.remove(&format!(
        "display-data-{}",
        header.transcript.pages.last().unwrap().hash
    ));
    assert!(store.saved_display_page().is_none());
    let view = store.project_read_view(full, append).unwrap();
    assert_eq!(view.transcript.len(), 50);
    assert!(store.fresh_read_model().is_some());
    backend.append(1);
    backend.0.lock().unwrap().fail_head = true;
    let view = store.project_read_view(full, append).unwrap();
    assert_eq!(view.summary.message_count, 76);
    assert_eq!(view.transcript.len(), 76);
    assert!(view.cursor.is_none());
    assert!(store.fresh_read_model().is_none());
}

#[test]
fn cache_pages_have_a_byte_budget_and_tool_index_repairs_without_journal_scan() {
    use proteus_contracts::app_protocol::AppTranscriptTool;
    let (backend, store) = Backend::store();
    backend.append(4);
    let mut model = SessionReadModel::from_validated(
        &store,
        RemoteValidatedRead::open(store.clone()).unwrap(),
        full,
    )
    .unwrap();
    for row in &mut model.transcript {
        row.text = "x".repeat(2 * 1024 * 1024);
    }
    model.transcript.last_mut().unwrap().tool = Some(AppTranscriptTool {
        terminal: None,
        call_id: "one-tool".into(),
        name: "edit".into(),
        args: serde_json::json!({"path":"a"}),
        effective_args: None,
        status: "completed".into(),
        result: Some("large result".repeat(100)),
        metadata: serde_json::Value::Null,
    });
    let source = RemoteCacheSource::read(&store).unwrap();
    store.save_read_model(&source, &model);
    let header = cache::header(&store).unwrap();
    assert!(
        header.transcript.pages.len() >= 2,
        "byte limits must split a small number of large rows"
    );
    let tool = header.tools.get("edit").unwrap();
    backend
        .0
        .lock()
        .unwrap()
        .cache
        .remove(&format!("display-data-{tool}"));
    let reads = backend.0.lock().unwrap().record_reads;
    let state = store.saved_tool_state("edit").unwrap().tool.unwrap();
    assert_eq!(state.call_id, "one-tool");
    assert_eq!(state.args["path"], "a");
    assert_eq!(
        backend.0.lock().unwrap().record_reads,
        reads,
        "compact index repair uses verified cached rows only"
    );
}

#[test]
fn cache_fingerprints_ignore_independent_worker_object_order() {
    let first: serde_json::Value = serde_json::from_str(r#"{"b":2,"a":{"z":0,"x":1}}"#).unwrap();
    let second: serde_json::Value = serde_json::from_str(r#"{"a":{"x":1,"z":0},"b":2}"#).unwrap();
    assert_eq!(
        proteus_contracts::contracts::storage_value_etag(&first).unwrap(),
        proteus_contracts::contracts::storage_value_etag(&second).unwrap()
    );
    let (backend, store) = Backend::store();
    backend.append(51);
    store.project_read_view(full, append).unwrap();
    for value in backend.0.lock().unwrap().cache.values_mut() {
        value.sort_all_objects();
    }
    assert_eq!(store.saved_transcript_page(None, None).unwrap().total, 51);
    assert!(store.fresh_read_model().is_some());
}

mod latest_page;
mod packs;

#[test]
fn full_reprojection_compares_changed_archived_leaf_and_preserves_pinned_history() {
    let (backend, store) = Backend::store();
    backend.append(953);
    let original = store.project_read_model(full).unwrap();
    let old_header = cache::header(&store).unwrap();
    let old_revision = old_header.source.revision().unwrap();
    let changed = 401usize;
    backend.append(1);
    let mut current = SessionReadModel::from_validated(
        &store,
        RemoteValidatedRead::open(store.clone()).unwrap(),
        full,
    )
    .unwrap();
    current.transcript[changed].text = "replacement".to_string();
    let source = RemoteCacheSource::read(&store).unwrap();
    store.save_read_model(&source, &current);
    let current_page = store.saved_transcript_page(Some(425), None).unwrap();
    let base = current_page.base.unwrap();
    assert_eq!(base.revision, old_revision);
    assert_eq!(base.common_prefix, changed);
    assert_eq!(
        current_page.messages[changed - current_page.start].text,
        "replacement"
    );
    let pinned = store
        .saved_transcript_page(Some(425), Some(&old_revision))
        .unwrap();
    assert_eq!(
        pinned.messages[changed - pinned.start].text,
        original.transcript[changed].text
    );
    // Evicting only the old pack must never resolve this revision against the
    // newer pack, even though both manifests refer to leaf index 8.
    let old = serde_json::to_value(old_header.transcript).unwrap();
    let digest = old["packs"][1]["hash"].as_str().unwrap();
    backend
        .0
        .lock()
        .unwrap()
        .cache
        .remove(&format!("display-data-{digest}"));
    assert!(
        store
            .saved_transcript_page(Some(425), Some(&old_revision))
            .is_none()
    );
    assert!(store.saved_transcript_page(Some(425), None).is_some());
}

mod startup_save;
