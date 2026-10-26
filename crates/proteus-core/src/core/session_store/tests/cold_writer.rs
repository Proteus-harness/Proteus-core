//! Cold recovery must retain canonical identity checks for the next append.
use super::*;

#[tokio::test]
async fn recovered_writer_rejects_changed_parts_and_commits_a_following_valid_append() {
    let config = tempfile::tempdir().expect("config");
    let workspace = tempfile::tempdir().expect("workspace");
    let store = test_store(config.path(), workspace.path());
    let thread = new_thread_id();
    let original = CanonicalMessage::text(MessageRole::User, "committed identity");
    store
        .append_history(thread, None, &[original.clone()])
        .await
        .expect("seed");
    let path = store.session_dir().to_owned();
    drop(store);

    let reopened = SessionStore::open(path).expect("cold store");
    let before = std::fs::read(reopened.journal_path()).expect("journal");
    let mut changed = CanonicalMessage::text(MessageRole::Assistant, "forged content");
    changed.parts[0].part_id = original.parts[0].part_id;
    let error = reopened
        .append_history(thread, None, &[changed])
        .await
        .expect_err("recovered writer retains known part identities");
    assert!(error.to_string().contains("part"), "{error:#}");
    assert_eq!(
        std::fs::read(reopened.journal_path()).expect("unchanged journal"),
        before
    );

    let valid = CanonicalMessage::text(MessageRole::Assistant, "valid continuation");
    reopened
        .append_history(thread, None, &[valid.clone()])
        .await
        .expect("rejected candidate did not poison recovered validation");
    let projection = reopened.load_projection().expect("strict cold projection");
    assert_eq!(projection.history, vec![original, valid]);
    assert_eq!(projection.history_revision, 2);
    assert_eq!(projection.records.len(), 2);
    assert_eq!(projection.records[1].session_seq, 2);
}
