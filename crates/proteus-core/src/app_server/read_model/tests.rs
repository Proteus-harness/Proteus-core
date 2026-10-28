use super::*;
use crate::{
    domain::{new_session_id, new_thread_id},
    model_standard::{CanonicalMessage, MessageRole},
};
use proteus_contracts::app_protocol::AppTranscriptPage;

async fn fixture(
    count: usize,
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    SessionStore,
    AppTranscriptPage,
) {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path(), workspace.path(), new_session_id()).unwrap();
    let thread = new_thread_id();
    let messages: Vec<_> = (0..count.max(1))
        .map(|row| CanonicalMessage::text(MessageRole::User, format!("row {row}")))
        .collect();
    store.append_history(thread, None, &messages).await.unwrap();
    if count == 0 {
        store.clear_history(thread).await.unwrap();
    }
    store
        .project_read_model(|projection| journal_transcript_messages(projection, None))
        .unwrap();
    let page = store.saved_transcript_page(None, None).unwrap();
    (root, workspace, store, page)
}

#[tokio::test]
async fn history_recovery_covers_empty_short_and_exact_page_boundaries() {
    for count in [0usize, 1, 49, 50, 51, 100, 121] {
        let (_root, _workspace, store, old) = fixture(count).await;
        let journal = std::fs::read(store.journal_path()).unwrap();
        std::fs::remove_file(store.session_dir().join(".read-view.sqlite")).unwrap();
        let mut page = transcript_page(&store, None, Some(&old.revision)).unwrap();
        assert_eq!(page.revision, old.revision);
        assert_eq!(page.total, count);
        assert_eq!(page.messages.len(), count.min(50));
        assert_eq!(page.start, count.saturating_sub(50));
        let mut rows = page.messages;
        while page.start > 0 {
            page = transcript_page(&store, Some(page.start), Some(&old.revision)).unwrap();
            let mut prefix = page.messages;
            prefix.extend(rows);
            rows = prefix;
        }
        assert_eq!(
            rows.iter()
                .map(|message| message.text.clone())
                .collect::<Vec<_>>(),
            (0..count)
                .map(|row| format!("row {row}"))
                .collect::<Vec<_>>()
        );
        assert!(transcript_page(&store, Some(count + 1), Some(&old.revision)).is_err());
        assert_eq!(std::fs::read(store.journal_path()).unwrap(), journal);
    }
}

#[tokio::test]
async fn history_recovery_never_relabels_new_source_as_a_lost_revision() {
    let (_root, _workspace, store, old) = fixture(121).await;
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(MessageRole::User, "new row")],
        )
        .await
        .unwrap();
    let journal = std::fs::read(store.journal_path()).unwrap();
    // A surviving completed display still addresses the old immutable rows.
    assert_eq!(
        transcript_at_revision(&store, &old.revision).unwrap().len(),
        121
    );
    assert_eq!(
        transcript_page(&store, Some(71), Some(&old.revision))
            .unwrap()
            .total,
        121
    );
    let index = store.session_dir().join(".read-view.sqlite");
    std::fs::remove_file(&index).unwrap();
    assert!(transcript_at_revision(&store, &old.revision).is_err());
    assert!(transcript_page(&store, Some(71), Some(&old.revision)).is_err());
    assert!(
        !index.exists(),
        "a rejected old revision must not trigger a rebuild"
    );
    let current = transcript_page(&store, None, None).unwrap();
    assert_ne!(current.revision, old.revision);
    assert_eq!(current.total, 122);
    assert_eq!(current.messages.last().unwrap().text, "new row");
    assert_eq!(std::fs::read(store.journal_path()).unwrap(), journal);
}

#[tokio::test]
async fn history_recovery_does_not_hide_corrupt_canonical_source() {
    let (_root, _workspace, store, old) = fixture(51).await;
    let index = store.session_dir().join(".read-view.sqlite");
    std::fs::remove_file(&index).unwrap();
    std::fs::write(store.journal_path(), b"{broken complete record}\n").unwrap();
    let damaged = std::fs::read(store.journal_path()).unwrap();
    assert!(transcript_at_revision(&store, &old.revision).is_err());
    assert!(transcript_page(&store, None, None).is_err());
    assert!(!index.exists());
    assert_eq!(std::fs::read(store.journal_path()).unwrap(), damaged);
}
