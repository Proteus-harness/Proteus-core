use super::*;
use crate::{
    core::SessionStore,
    domain::{new_session_id, new_thread_id},
    model_standard::{CanonicalMessage, MessageRole},
};

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
