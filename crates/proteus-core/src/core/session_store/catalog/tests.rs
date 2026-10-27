use std::{collections::BTreeMap, io::Write};

use crate::{
    core::{CONFIG_SNAPSHOT_FILE, JOURNAL_SCHEMA_VERSION, SessionStore},
    domain::{new_session_id, new_thread_id},
};

use super::*;

const SESSION_METADATA_FILE: &str = "session.json";

async fn saved_store(root: &Path, workspace: &Path, preview: &str) -> SessionStore {
    let store = SessionStore::new(root, workspace, new_session_id()).unwrap();
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(MessageRole::User, preview)],
        )
        .await
        .unwrap();
    store
}

fn file_bytes(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        // Disposable read models may be created; canonical files stay byte-exact.
        if matches!(
            entry.file_name().to_str(),
            Some(
                ".read-summary.sqlite"
                    | ".read-view.sqlite"
                    | ".read-summary.lock"
                    | ".read-view.lock"
            )
        ) {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            files.extend(file_bytes(&entry.path()));
        } else {
            files.insert(entry.path(), std::fs::read(entry.path()).unwrap());
        }
    }
    files
}

fn change_metadata(store: &SessionStore, key: &str, value: serde_json::Value) {
    let path = store.session_dir().join(SESSION_METADATA_FILE);
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    metadata[key] = value;
    std::fs::write(path, serde_json::to_vec(&metadata).unwrap()).unwrap();
}

#[tokio::test]
async fn catalog_isolates_incompatible_sessions_and_preserves_all_bytes() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let other_workspace = tempfile::tempdir().unwrap();
    let current = saved_store(root.path(), workspace.path(), "current history").await;
    let other = saved_store(root.path(), other_workspace.path(), "other workspace").await;
    let old_metadata = saved_store(root.path(), workspace.path(), "old metadata").await;
    // Unsupported metadata must be classified before any current DTO is decoded.
    std::fs::write(
        old_metadata.session_dir().join(SESSION_METADATA_FILE),
        br#"{"schema_version":3,"old_shape":true}"#,
    )
    .unwrap();
    let old_journal_metadata = saved_store(root.path(), workspace.path(), "old journal tag").await;
    change_metadata(
        &old_journal_metadata,
        "journal_schema_version",
        serde_json::json!(JOURNAL_SCHEMA_VERSION - 1),
    );
    let old_journal = saved_store(root.path(), workspace.path(), "old record").await;
    // Leave a current first record, followed by an old shape the strict DTO cannot decode.
    let mut journal = std::fs::OpenOptions::new()
        .append(true)
        .open(old_journal.journal_path())
        .unwrap();
    writeln!(
        journal,
        "{{\"schema_version\":{},\"old_shape\":true}}",
        JOURNAL_SCHEMA_VERSION - 1
    )
    .unwrap();
    drop(journal);
    let future_journal = saved_store(root.path(), workspace.path(), "future record").await;
    let mut future_record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(future_journal.journal_path()).unwrap()).unwrap();
    future_record["schema_version"] = serde_json::json!(JOURNAL_SCHEMA_VERSION + 1);
    let mut future_bytes = serde_json::to_vec(&future_record).unwrap();
    future_bytes.push(b'\n');
    std::fs::write(future_journal.journal_path(), future_bytes).unwrap();
    let uuid_dir = current
        .session_dir()
        .parent()
        .unwrap()
        .join(new_session_id().to_string());
    std::fs::create_dir(&uuid_dir).unwrap();
    std::fs::write(uuid_dir.join("messages.jsonl"), b"old history\n").unwrap();
    // Startup uses the current config, not the persisted config snapshot reader.
    std::fs::write(
        current.session_dir().join(CONFIG_SNAPSHOT_FILE),
        br#"{"schema_version":0,"old_shape":true}"#,
    )
    .unwrap();
    let before = file_bytes(&root.path().join("sessions"));

    let summaries = list_workspace_session_summaries(root.path(), workspace.path()).unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].session_id, current.session_id());
    assert_eq!(summaries[0].message_count, 1);
    assert_eq!(summaries[0].preview.as_deref(), Some("current history"));
    let all = list_session_summaries(root.path()).unwrap();
    assert_eq!(all.len(), 2);
    assert!(
        all.iter()
            .any(|summary| summary.session_id == other.session_id())
    );
    let metadata_error = SessionStore::open(old_metadata.session_dir().to_path_buf()).unwrap_err();
    assert!(
        metadata_error
            .to_string()
            .contains("unsupported session schema_version 3")
    );
    let journal_tag_error =
        SessionStore::open(old_journal_metadata.session_dir().to_path_buf()).unwrap_err();
    assert!(
        journal_tag_error
            .to_string()
            .contains("unsupported journal_schema_version")
    );
    for store in [&old_journal, &future_journal] {
        let opened = SessionStore::open(store.session_dir().to_path_buf()).unwrap();
        assert!(
            opened.load_projection().is_err(),
            "strict journal validation"
        );
    }
    let version_error = future_journal.load_projection().unwrap_err();
    assert!(format!("{version_error:#}").contains("unsupported journal schema_version"));
    assert!(SessionStore::open(uuid_dir).is_err());
    assert!(list_session_summaries_for_audit(root.path(), Some(workspace.path())).is_err());
    assert_eq!(file_bytes(&root.path().join("sessions")), before);
}

#[tokio::test]
async fn catalog_isolates_current_corruption_but_audit_reports_it() {
    let workspace = tempfile::tempdir().unwrap();
    for corruption in ["metadata", "journal", "identity", "missing", "io"] {
        let root = tempfile::tempdir().unwrap();
        let store = saved_store(root.path(), workspace.path(), "corrupt session").await;
        match corruption {
            "metadata" => change_metadata(&store, "unknown_current_field", serde_json::json!(true)),
            "journal" => std::fs::write(store.journal_path(), b"{broken}\n").unwrap(),
            "identity" => {
                change_metadata(&store, "session_id", serde_json::json!(new_session_id()))
            }
            "missing" => {
                std::fs::remove_file(store.session_dir().join(SESSION_METADATA_FILE)).unwrap()
            }
            "io" => {
                std::fs::remove_file(store.journal_path()).unwrap();
                std::fs::create_dir(store.journal_path()).unwrap();
            }
            _ => unreachable!(),
        }
        let before = file_bytes(store.session_dir());
        assert!(
            list_workspace_session_summaries(root.path(), workspace.path())
                .unwrap()
                .is_empty()
        );
        assert!(list_session_summaries_for_audit(root.path(), Some(workspace.path())).is_err());
        assert!(
            SessionStore::open(store.session_dir().to_path_buf())
                .and_then(|s| s.load_messages())
                .is_err()
        );
        assert_eq!(file_bytes(store.session_dir()), before);
    }
}

#[tokio::test]
async fn catalog_keeps_journal_interrupted_tail_semantics() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved_store(root.path(), workspace.path(), "committed").await;
    let mut journal = std::fs::OpenOptions::new()
        .append(true)
        .open(store.journal_path())
        .unwrap();
    write!(journal, "{{\"schema_version\":0").unwrap();
    drop(journal);
    let before = file_bytes(store.session_dir());
    assert_eq!(list_session_summaries(root.path()).unwrap().len(), 1);
    assert_eq!(file_bytes(store.session_dir()), before);
}

#[test]
fn catalog_propagates_root_directory_errors() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let sessions = root.path().join("sessions");
    std::fs::write(&sessions, b"not a directory").unwrap();
    assert!(list_session_summaries(root.path()).is_err());
    assert!(list_workspace_session_summaries(root.path(), workspace.path()).is_err());
    assert!(prepare_latest_workspace_session(root.path(), workspace.path()).is_err());
    assert!(list_session_summaries_for_audit(root.path(), None).is_err());
    std::fs::remove_file(&sessions).unwrap();
    std::fs::create_dir(&sessions).unwrap();
    let workspace_dir = sessions.join(encode_workspace_path(workspace.path()).unwrap());
    std::fs::write(&workspace_dir, b"not a directory").unwrap();
    assert!(list_workspace_session_summaries(root.path(), workspace.path()).is_err());
    assert!(prepare_latest_workspace_session(root.path(), workspace.path()).is_err());
    assert!(list_session_summaries_for_audit(root.path(), Some(workspace.path())).is_err());
}

fn set_journal_time(store: &SessionStore, millis: u64) {
    let time = UNIX_EPOCH + std::time::Duration::from_millis(millis);
    std::fs::File::options()
        .write(true)
        .open(store.journal_path())
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(time))
        .unwrap();
}

#[tokio::test]
async fn prepared_latest_matches_catalog_millisecond_tie_order() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let first = saved_store(root.path(), workspace.path(), "first").await;
    let second = saved_store(root.path(), workspace.path(), "second").await;
    let older = saved_store(root.path(), workspace.path(), "older").await;
    set_journal_time(&first, 10_000);
    set_journal_time(&second, 10_000);
    set_journal_time(&older, 9_999);
    let expected = list_workspace_session_summaries(root.path(), workspace.path()).unwrap();
    assert_eq!(
        expected[0].session_dir,
        first.session_dir().max(second.session_dir())
    );
    let prepared = prepare_latest_workspace_session(root.path(), workspace.path())
        .unwrap()
        .unwrap();
    assert_eq!(prepared.session_dir(), expected[0].session_dir);
    assert_eq!(prepared.workspace_path().unwrap(), workspace.path());
    let (store, model, deferred, stale) = prepared.into_display_view(|_| Vec::new()).unwrap();
    assert_eq!(store.session_id(), expected[0].session_id);
    assert_eq!(
        model.summary.message_count,
        store.load_messages().unwrap().len()
    );
    assert!(!deferred && !stale);
}

#[tokio::test]
async fn prepared_latest_skips_newer_unusable_and_empty_sessions_without_canonical_writes() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let valid = saved_store(root.path(), workspace.path(), "usable").await;
    let corrupt = saved_store(root.path(), workspace.path(), "corrupt").await;
    let old = saved_store(root.path(), workspace.path(), "old").await;
    let empty = saved_store(root.path(), workspace.path(), "cleared").await;
    empty.clear_history(new_thread_id()).await.unwrap();
    std::fs::write(corrupt.journal_path(), b"{broken}\n").unwrap();
    change_metadata(&old, "schema_version", serde_json::json!(3));
    for (store, millis) in [
        (&valid, 1_000),
        (&corrupt, 2_000),
        (&old, 3_000),
        (&empty, 4_000),
    ] {
        set_journal_time(store, millis);
    }
    let before = file_bytes(&root.path().join("sessions"));
    let prepared = prepare_latest_workspace_session(root.path(), workspace.path())
        .unwrap()
        .unwrap();
    assert_eq!(prepared.session_dir(), valid.session_dir());
    for invalid in [&corrupt, &old] {
        assert!(PreparedSession::from_dir(invalid.session_dir().to_owned()).is_err());
    }
    assert!(
        PreparedSession::from_dir(empty.session_dir().to_owned())
            .unwrap()
            .message_count()
            == 0
    );
    assert_eq!(file_bytes(&root.path().join("sessions")), before);
}

#[tokio::test]
async fn prepared_latest_returns_none_for_missing_or_empty_workspace() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    assert!(
        prepare_latest_workspace_session(root.path(), workspace.path())
            .unwrap()
            .is_none()
    );
    let empty = saved_store(root.path(), workspace.path(), "cleared").await;
    empty.clear_history(new_thread_id()).await.unwrap();
    assert!(
        prepare_latest_workspace_session(root.path(), workspace.path())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        list_workspace_session_summaries(root.path(), workspace.path())
            .unwrap()
            .len(),
        0
    );
}
