use super::*;
use crate::{
    core::SessionStore,
    domain::{new_session_id, new_thread_id},
    model_standard::{CanonicalMessage, MessageRole},
};

#[test]
fn directory_cannot_be_mistaken_for_an_empty_journal() {
    let root = tempfile::tempdir().unwrap();
    let path = journal_path(root.path());
    std::fs::create_dir(&path).unwrap();
    assert!(
        load_records(root.path(), new_session_id())
            .unwrap_err()
            .to_string()
            .contains("not a regular file")
    );
    assert!(
        recovery::repair_unterminated_tail(&path)
            .unwrap_err()
            .to_string()
            .contains("not a regular file")
    );
}

#[tokio::test]
async fn blob_and_inline_decoders_preserve_records_and_reject_invalid_payloads() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path(), workspace.path(), new_session_id()).unwrap();
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(
                MessageRole::User,
                "synthetic payload".repeat(100),
            )],
        )
        .await
        .unwrap();
    let expected = store.load_records().unwrap();
    let mut stored: Vec<StoredJournalRecord> = std::fs::read_to_string(store.journal_path())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for record in &mut stored {
        let StoredPayload::Inline { value } = &record.payload else {
            panic!("inline fixture")
        };
        record.payload =
            write_blob(store.session_dir(), &serde_json::to_vec(value).unwrap()).unwrap();
    }
    write_records(&store, &stored);
    assert_eq!(store.load_records().unwrap(), expected);

    let mut malformed = expected[0].entry.payload_value().unwrap();
    assert!(
        malformed
            .as_object_mut()
            .unwrap()
            .remove("messages")
            .is_some()
    );
    for blob in [false, true] {
        stored[0].payload = if blob {
            write_blob(
                store.session_dir(),
                &serde_json::to_vec(&malformed).unwrap(),
            )
            .unwrap()
        } else {
            StoredPayload::Inline {
                value: malformed.clone(),
            }
        };
        write_records(&store, &stored);
        let error = store.load_records().unwrap_err();
        assert!(format!("{error:#}").contains("messages"), "{error:#}");
    }
}

fn write_records(store: &SessionStore, records: &[StoredJournalRecord]) {
    let mut bytes = Vec::new();
    for record in records {
        serde_json::to_writer(&mut bytes, record).unwrap();
        bytes.push(b'\n');
    }
    std::fs::write(store.journal_path(), bytes).unwrap();
}

#[tokio::test]
async fn snapshot_passes_keep_the_original_inode_and_complete_prefix() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path(), workspace.path(), new_session_id()).unwrap();
    let thread = new_thread_id();
    store
        .append_history(
            thread,
            None,
            &[CanonicalMessage::text(MessageRole::User, "first")],
        )
        .await
        .unwrap();
    let expected = store.load_records().unwrap();
    let mut snapshot = JournalReadSnapshot::open(store.session_dir(), store.session_id()).unwrap();
    store
        .append_history(
            thread,
            None,
            &[CanonicalMessage::text(MessageRole::Assistant, "later")],
        )
        .await
        .unwrap();
    for replace in [false, true] {
        if replace {
            std::fs::rename(
                store.journal_path(),
                store.session_dir().join("old-journal"),
            )
            .unwrap();
            std::fs::write(store.journal_path(), b"invalid replacement\n").unwrap();
        }
        let mut actual = Vec::new();
        snapshot
            .scan(|record| {
                actual.push(record);
                Ok(())
            })
            .unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn snapshot_of_missing_file_stays_empty_and_shortened_prefix_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let session = new_session_id();
    let mut missing = JournalReadSnapshot::open(root.path(), session).unwrap();
    std::fs::write(journal_path(root.path()), b"a captured complete line\n").unwrap();
    missing
        .scan(|_| panic!("missing snapshot cannot observe a later file"))
        .unwrap();
    let mut snapshot = JournalReadSnapshot::open(root.path(), session).unwrap();
    StdOpenOptions::new()
        .write(true)
        .open(journal_path(root.path()))
        .unwrap()
        .set_len(0)
        .unwrap();
    let error = snapshot.scan(|_| Ok(())).unwrap_err();
    assert!(
        error.to_string().contains("shortened during snapshot read"),
        "{error:#}"
    );
}

#[tokio::test]
async fn snapshot_rejects_valid_in_place_changes_before_successful_derived_return() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path(), workspace.path(), new_session_id()).unwrap();
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(MessageRole::User, "first")],
        )
        .await
        .unwrap();
    let mut snapshot = JournalReadSnapshot::open(store.session_dir(), store.session_id()).unwrap();
    assert!(
        snapshot
            .scan_selected(|_| true, |_| Ok(()))
            .unwrap_err()
            .to_string()
            .contains("requires a complete typed scan")
    );
    snapshot.scan(|_| Ok(())).unwrap();
    let original = std::fs::read_to_string(store.journal_path()).unwrap();
    let changed = original.replace("first", "other");
    assert_ne!(original, changed);
    assert_eq!(original.len(), changed.len());
    std::fs::write(store.journal_path(), changed).unwrap();
    let mut unpublished = Vec::new();
    let error = snapshot
        .scan_selected(
            |kind| kind == JournalKind::HistoryMutated,
            |record| {
                unpublished.push(record);
                Ok(())
            },
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("complete prefix changed"),
        "{error:#}"
    );
    assert_eq!(unpublished.len(), 1);
    std::fs::write(store.journal_path(), original).unwrap();
    snapshot.scan_selected(|_| true, |_| Ok(())).unwrap();
}
