//! Cold recovery must retain canonical identity checks for the next append.
use super::*;

#[tokio::test]
async fn cold_writer_keeps_retired_part_guards_and_resume_retains_all_records() {
    for threshold in [1, usize::MAX] {
        let config = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let mut store = test_store(config.path(), workspace.path());
        store.blob_threshold_bytes = threshold;
        let thread = new_thread_id();
        let retired = CanonicalMessage::text(MessageRole::User, "retired ".repeat(1024));
        let current = CanonicalMessage::text(MessageRole::User, "current ".repeat(1024));
        for message in [&retired, &current] {
            store
                .replace_history(thread, None, std::slice::from_ref(message), None)
                .await
                .unwrap();
        }
        let path = store.session_dir().to_owned();
        drop(store);
        let reopened = SessionStore::open(path.clone()).unwrap();
        let before = std::fs::read(reopened.journal_path()).unwrap();
        if threshold == 1 {
            // The obsolete first snapshot must still be hash-checked, even
            // though only the second snapshot remains in current history.
            let stored: serde_json::Value =
                serde_json::from_slice(before.split(|byte| *byte == b'\n').next().unwrap())
                    .unwrap();
            let blob = reopened
                .session_dir()
                .join(stored["payload"]["relative_path"].as_str().unwrap());
            let original = std::fs::read(&blob).unwrap();
            let mut damaged = original.clone();
            damaged[0] ^= 1;
            std::fs::write(&blob, &damaged).unwrap();
            let error = reopened
                .append_history(
                    thread,
                    None,
                    &[CanonicalMessage::text(
                        MessageRole::Assistant,
                        "blocked by old corruption",
                    )],
                )
                .await
                .unwrap_err();
            assert!(format!("{error:#}").contains("hash mismatch"));
            assert!(reopened.writer.lock().await.execution_state().is_none());
            assert_eq!(std::fs::read(reopened.journal_path()).unwrap(), before);
            std::fs::write(blob, original).unwrap();
        }
        let mut forged = retired.clone();
        forged.parts[0].payload = ContentPart::Text {
            text: "changed after retirement".into(),
        };
        assert!(
            reopened
                .append_history(thread, None, &[forged])
                .await
                .unwrap_err()
                .to_string()
                .contains("changed after it was recorded")
        );
        assert_eq!(std::fs::read(reopened.journal_path()).unwrap(), before);
        let continuation = CanonicalMessage::text(MessageRole::Assistant, "continued");
        reopened
            .append_history(thread, None, std::slice::from_ref(&continuation))
            .await
            .unwrap();
        let strict = reopened.load_projection().unwrap();
        assert_eq!(strict.history, vec![current, continuation]);
        assert_eq!(strict.history_revision, 3);
        drop(reopened);
        let resumed = SessionStore::open(path)
            .unwrap()
            .prepare_resume_projection()
            .await
            .unwrap();
        assert_eq!(resumed.records, strict.records);
        assert_eq!(resumed.history, strict.history);
        assert_eq!(resumed.history_revision, strict.history_revision);
    }
}

#[tokio::test]
async fn failed_late_recovery_does_not_publish_partial_validation_or_consume_ids() {
    let config = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = test_store(config.path(), workspace.path());
    let thread = new_thread_id();
    for index in 0..3 {
        store
            .append_history(
                thread,
                None,
                &[CanonicalMessage::text(
                    MessageRole::User,
                    format!("seed {index}"),
                )],
            )
            .await
            .unwrap();
    }
    let path = store.session_dir().to_owned();
    let journal = store.journal_path();
    let original = std::fs::read(&journal).unwrap();
    drop(store);
    let mut records: Vec<serde_json::Value> = original
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    records[2]["session_seq"] = serde_json::json!(2);
    let malformed: Vec<u8> = records
        .iter()
        .flat_map(|record| {
            let mut bytes = serde_json::to_vec(record).unwrap();
            bytes.push(b'\n');
            bytes
        })
        .collect();
    std::fs::write(&journal, &malformed).unwrap();
    let reopened = SessionStore::open(path).unwrap();
    let continuation = CanonicalMessage::text(MessageRole::Assistant, "valid after repair");
    assert!(
        reopened
            .append_history(thread, None, std::slice::from_ref(&continuation))
            .await
            .unwrap_err()
            .to_string()
            .contains("sequence mismatch")
    );
    assert!(reopened.writer.lock().await.execution_state().is_none());
    assert_eq!(std::fs::read(&journal).unwrap(), malformed);
    std::fs::write(&journal, &original).unwrap();
    reopened
        .append_history(thread, None, std::slice::from_ref(&continuation))
        .await
        .unwrap();
    let strict = reopened.load_projection().unwrap();
    assert_eq!(strict.history.last(), Some(&continuation));
    assert_eq!(strict.history_revision, 4);
    assert_eq!(strict.records.last().unwrap().session_seq, 4);
}

#[tokio::test]
async fn read_only_torn_tail_is_ignored_then_cold_writer_repairs_it() {
    let config = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = test_store(config.path(), workspace.path());
    let thread = new_thread_id();
    let first = CanonicalMessage::text(MessageRole::User, "first");
    store
        .append_history(thread, None, std::slice::from_ref(&first))
        .await
        .unwrap();
    let path = store.session_dir().to_owned();
    let journal = store.journal_path();
    let mut interrupted = std::fs::read(&journal).unwrap();
    interrupted.extend(std::iter::repeat_n(0xff, 131073));
    std::fs::write(&journal, &interrupted).unwrap();
    drop(store);
    let reopened = SessionStore::open(path).unwrap();
    assert_eq!(reopened.load_messages().unwrap(), vec![first.clone()]);
    assert_eq!(std::fs::read(&journal).unwrap(), interrupted);
    let second = CanonicalMessage::text(MessageRole::Assistant, "second");
    reopened
        .append_history(thread, None, std::slice::from_ref(&second))
        .await
        .unwrap();
    let strict = reopened.load_projection().unwrap();
    assert_eq!(strict.history, vec![first, second]);
    assert_eq!(strict.records.len(), 2);
    assert_eq!(strict.records[1].session_seq, 2);
}

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
