use super::*;
use proteus_contracts::app_protocol::AppTranscriptTool;
use proteus_contracts::contracts::ModelContextObservation;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn capture_io(backend: &Backend) -> (Vec<String>, Vec<String>, BTreeMap<String, Vec<u8>>) {
    let mut state = backend.0.lock().unwrap();
    (
        std::mem::take(&mut state.cache_reads),
        std::mem::take(&mut state.writes),
        state
            .cache
            .iter()
            .map(|(key, value)| (key.clone(), serde_json::to_vec(value).unwrap()))
            .collect(),
    )
}

#[test]
fn startup_save_preserves_borrowed_cache_bytes_and_operations() {
    for fail_head in [false, true] {
        let (backend, store) = Backend::store();
        backend.append(101);
        store.project_read_model(full).unwrap();
        backend.append(50);
        let read = RemoteValidatedRead::open(store.clone()).unwrap();
        let source = RemoteCacheSource::from_cursor(&read.cursor());
        let mut model = SessionReadModel::from_validated(&store, read, full).unwrap();
        model.conversation.model_context = vec![ModelContextObservation::Usage {
            total_tokens: 42,
            last_tokens: 17,
        }];
        model.transcript[140].tool = Some(AppTranscriptTool {
            terminal: None,
            call_id: "fixture-call".into(),
            name: "fixture-tool".into(),
            args: json!({"path": "é"}),
            effective_args: None,
            status: "completed".into(),
            result: None,
            metadata: Value::Null,
        });
        let initial = {
            let mut state = backend.0.lock().unwrap();
            state.writes.clear();
            state.cache_reads.clear();
            state.fail_head = fail_head;
            state.cache.clone()
        };
        let borrowed = cache::save_model(&store, &source, &model, None);
        let old = capture_io(&backend);
        backend.0.lock().unwrap().cache = initial;
        let owned = cache::save_startup_model(&store, &source, &mut model);
        let new = capture_io(&backend);
        assert_eq!(old, new);
        assert_eq!(owned, borrowed);
        assert_eq!(owned.is_none(), fail_head);
        assert!(model.conversation.messages.is_empty());
        assert!(model.conversation.model_context.is_empty());
        assert_eq!(model.transcript.len(), 151);
    }
}

#[test]
fn failed_head_publication_keeps_the_completed_named_startup_tail() {
    let (backend, store) = Backend::store();
    backend.append(80);
    let read = RemoteValidatedRead::open(store.clone()).unwrap();
    let revision = RemoteCacheSource::from_cursor(&read.cursor())
        .revision()
        .unwrap();
    backend.0.lock().unwrap().fail_head = true;
    let view = store.project_validated_view(read, true, full).unwrap();
    assert_eq!(view.summary.message_count, 80);
    let cursor = view.cursor.expect("named immutable revision was written");
    assert_eq!(cursor.revision, revision);
    assert_eq!(cursor.before, 30);
    assert_eq!(view.transcript.len(), 50);
    assert_eq!(view.transcript[0].text, "message-0-30");
    assert!(store.saved_display_page().is_none());
}

#[test]
fn invalid_render_never_enters_startup_save() {
    let (backend, store) = Backend::store();
    backend.append(80);
    let read = RemoteValidatedRead::open(store.clone()).unwrap();
    let err = store
        .project_validated_view(read, true, |_| anyhow::bail!("render sentinel"))
        .unwrap_err();
    assert_eq!(err.to_string(), "render sentinel");
    let state = backend.0.lock().unwrap();
    assert!(state.cache.is_empty());
    assert!(state.writes.is_empty());
    assert!(state.cache_reads.is_empty());
}

#[test]
fn stale_startup_with_a_missing_or_corrupt_named_tail_keeps_full_render() {
    for corrupt in [false, true] {
        let (backend, store) = Backend::store();
        backend.append(80);
        store.project_read_model(full).unwrap();
        let header = cache::header(&store).unwrap();
        let key = format!(
            "display-data-{}",
            header.transcript.pages.last().unwrap().hash
        );
        {
            let mut state = backend.0.lock().unwrap();
            assert!(state.cache.remove(&key).is_some());
            if corrupt {
                state.cache.insert(key, json!([null]));
            }
        }
        let read = RemoteValidatedRead::open(store.clone()).unwrap();
        // Its old named header still exists, but the backend now rejects
        // writes bound to the prepared snapshot and cannot repair that tail.
        backend.append(1);
        let view = store.project_validated_view(read, true, full).unwrap();
        assert_eq!(view.revision, 1);
        assert_eq!(view.summary.message_count, 80);
        assert_eq!(view.transcript.len(), 80);
        assert_eq!(view.transcript.last().unwrap().text, "message-0-79");
        assert!(view.cursor.is_none());
    }
}
