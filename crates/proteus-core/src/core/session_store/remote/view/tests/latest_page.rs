//! Latest reads use only the page and a final current-source witness on a hit.
use super::*;

#[test]
fn latest_reads_one_source_and_pinned_reads_none() {
    let (backend, store) = Backend::store();
    backend.append(121);
    store.project_read_model(full).unwrap();
    let mut state = backend.0.lock().unwrap();
    let (opens, records) = (state.handles, state.record_reads);
    state.cache_reads.clear();
    state.writes.clear();
    drop(state);

    let page = store.saved_transcript_page(Some(57), None).unwrap();
    assert_eq!((page.start, page.total), (7, 121));
    assert_eq!(page.messages.first().unwrap().text, "message-0-7");
    assert_eq!(page.messages.last().unwrap().text, "message-0-56");
    let pinned = store
        .saved_transcript_page(Some(57), Some(&page.revision))
        .unwrap();
    assert_eq!(
        serde_json::to_value(page).unwrap(),
        serde_json::to_value(pinned).unwrap()
    );
    let state = backend.0.lock().unwrap();
    assert_eq!(state.handles - opens, 1);
    assert_eq!(state.record_reads, records);
    assert!(state.writes.is_empty());
}

#[test]
fn stale_head_uses_the_named_current_index_without_replay_and_rechecks_after_read() {
    let (backend, store) = Backend::store();
    backend.append(51);
    store.project_read_model(full).unwrap();
    let first = store.saved_transcript_page(None, None).unwrap();
    backend.append(1);
    backend.0.lock().unwrap().fail_head = true;
    store.project_read_model(full).unwrap();
    let source = RemoteCacheSource::read(&store).unwrap();
    let revision = source.revision().unwrap();
    let current_header = cache::header_at_revision(&store, &revision).unwrap();
    let leaf = format!(
        "display-data-{}",
        current_header.transcript.pages.last().unwrap().hash
    );
    let first_header = cache::header(&store).unwrap();
    assert_ne!(
        current_header.transcript.pages.last().unwrap().hash,
        first_header.transcript.pages.last().unwrap().hash
    );
    assert_eq!(first_header.source.revision().unwrap(), first.revision);

    let mut state = backend.0.lock().unwrap();
    state.writes.clear();
    let records = state.record_reads;
    drop(state);
    let page = store.saved_transcript_page(None, None).unwrap();
    let pinned = store.saved_transcript_page(None, Some(&revision)).unwrap();
    assert_eq!(page.revision, revision);
    assert_eq!((page.start, page.total), (2, 52));
    assert_eq!(page.messages.last().unwrap().text, "message-1-0");
    assert_eq!(
        serde_json::to_value(page).unwrap(),
        serde_json::to_value(pinned).unwrap()
    );
    // A second append lands as the named index's body is read. It was current
    // at lookup but is no longer an answer to the implicit latest request.
    backend.0.lock().unwrap().append_on_cache_read = Some(leaf);
    assert!(store.saved_transcript_page(None, None).is_none());
    assert_eq!(
        store
            .saved_transcript_page(None, Some(&first.revision))
            .unwrap()
            .total,
        51
    );
    let state = backend.0.lock().unwrap();
    assert!(state.append_on_cache_read.is_none());
    assert_eq!(state.record_reads, records);
    assert!(state.writes.is_empty());
}

#[test]
fn a_source_change_during_the_head_page_read_is_not_returned_as_latest() {
    let (backend, store) = Backend::store();
    backend.append(51);
    store.project_read_model(full).unwrap();
    let head = cache::header(&store).unwrap();
    backend.0.lock().unwrap().append_on_cache_read = Some(format!(
        "display-data-{}",
        head.transcript.pages.last().unwrap().hash
    ));
    assert!(store.saved_transcript_page(None, None).is_none());
    assert!(backend.0.lock().unwrap().append_on_cache_read.is_none());
    assert_eq!(
        store
            .saved_transcript_page(None, Some(&head.source.revision().unwrap()))
            .unwrap()
            .total,
        51
    );
}
