use super::*;
use crate::domain::{new_session_id, new_thread_id};
use std::fs::{OpenOptions, TryLockError};
use std::time::Duration;

#[tokio::test]
async fn last_store_drop_releases_writer_state_and_lease_before_reopen() {
    let config = tempfile::tempdir().expect("config");
    let workspace = tempfile::tempdir().expect("workspace");
    let store =
        SessionStore::new(config.path(), workspace.path(), new_session_id()).expect("store");
    let thread_id = new_thread_id();
    let original = CanonicalMessage::text(MessageRole::User, "before releasing the writer");
    store
        .append_history(thread_id, None, &[original.clone()])
        .await
        .expect("first append");
    let session_dir = store.session_dir().to_owned();
    let peer = SessionStore::open(session_dir.clone()).expect("second handle");
    let writer = Arc::downgrade(&store.writer);
    assert!(Arc::ptr_eq(&store.writer, &peer.writer));
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .open(session_dir.join("journal.write.lock"))
        .expect("persistent lease file");

    drop(store);
    assert!(
        writer.upgrade().is_some(),
        "another store still owns the writer"
    );
    assert!(matches!(lease.try_lock(), Err(TryLockError::WouldBlock)));
    drop(peer);
    assert!(
        writer.upgrade().is_none(),
        "the registry must not retain validated history after the last store"
    );
    lease
        .try_lock()
        .expect("last store releases OS writer ownership");
    drop(lease);

    let reopened = SessionStore::open(session_dir).expect("reopen released session");
    let following = CanonicalMessage::text(MessageRole::Assistant, "after reopening");
    reopened
        .append_history(thread_id, None, &[following.clone()])
        .await
        .expect("recover and append");
    let projection = reopened.load_projection().expect("cold projection");
    assert_eq!(projection.history, vec![original, following]);
    assert_eq!(projection.history_revision, 2);
    assert_eq!(
        projection
            .records
            .iter()
            .map(|record| record.session_seq)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[tokio::test]
async fn reopening_waits_for_retiring_writer_without_blocking_other_sessions() {
    let config = tempfile::tempdir().expect("config");
    let workspace = tempfile::tempdir().expect("workspace");
    let store =
        SessionStore::new(config.path(), workspace.path(), new_session_id()).expect("store");
    let thread_id = new_thread_id();
    let original = CanonicalMessage::text(MessageRole::User, "before retiring");
    store
        .append_history(thread_id, None, &[original.clone()])
        .await
        .expect("first append");
    let session_dir = store.session_dir().to_owned();
    // Model a destructor paused after the final strong reference expires and
    // before its final lease/completion fields drop; no large fixture or race.
    let retirement = store.writer.lock().await.detach_retirement();
    let writer = Arc::downgrade(&store.writer);
    drop(store);
    assert!(writer.upgrade().is_none());

    // Opening another path prunes the registry. It must retain the unfinished
    // retirement entry even though its old writer can no longer be upgraded.
    let _other = SessionStore::new(config.path(), workspace.path(), new_session_id())
        .expect("unrelated session before reopen");
    let (entered, started) = tokio::sync::oneshot::channel();
    let mut reopening = tokio::task::spawn_blocking(move || {
        let _ = entered.send(());
        SessionStore::open(session_dir)
    });
    tokio::time::timeout(Duration::from_secs(5), started)
        .await
        .expect("reopen worker starts")
        .expect("reopen entered");
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut reopening)
            .await
            .is_err(),
        "a replacement must wait until the previous lease has closed"
    );
    let other_config = config.path().to_owned();
    let other_workspace = workspace.path().to_owned();
    let unrelated = tokio::task::spawn_blocking(move || {
        SessionStore::new(&other_config, &other_workspace, new_session_id())
    });
    tokio::time::timeout(Duration::from_secs(5), unrelated)
        .await
        .expect("retirement does not hold the global registry mutex")
        .expect("unrelated worker")
        .expect("unrelated session while reopening waits");

    // The detached tail drops its ownership before publishing completion.
    drop(retirement);
    let reopened = tokio::time::timeout(Duration::from_secs(5), reopening)
        .await
        .expect("replacement resumes after teardown")
        .expect("reopen worker")
        .expect("reopen session");
    let following = CanonicalMessage::text(MessageRole::Assistant, "after retiring");
    reopened
        .append_history(thread_id, None, &[following.clone()])
        .await
        .expect("replacement acquires the released lease");
    assert_eq!(
        reopened.load_messages().expect("history"),
        vec![original, following]
    );
}
