use serde_json::Value;
use std::collections::BTreeMap;

use super::*;

async fn saved_startup_store(root: &Path, workspace: &Path, text: &str) -> SessionStore {
    let store = SessionStore::new(root, workspace, new_session_id()).unwrap();
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(MessageRole::User, text)],
        )
        .await
        .unwrap();
    store
}

#[cfg(unix)]
#[tokio::test]
async fn stale_saved_transcript_refreshes_through_the_session_stream_without_a_turn() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let config = root.path().join("config.toml");
    let store = saved_startup_store(root.path(), workspace.path(), "old display").await;
    let first = AgentAppServer::launch_resumed(
        crate::test_model::config(),
        workspace.path().into(),
        Some(&config),
        store.session_dir().into(),
    )
    .await
    .unwrap();
    first.shutdown().await;
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(MessageRole::User, "external update")],
        )
        .await
        .unwrap();
    let before = std::fs::read(store.journal_path()).unwrap();
    let reopened = AgentAppServer::launch_resumed(
        crate::test_model::config(),
        workspace.path().into(),
        Some(&config),
        store.session_dir().into(),
    )
    .await
    .unwrap();
    let mut stream = reopened.subscribe_session();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let AppServerEvent::SessionSnapshot { snapshot } = stream.recv().await.unwrap()
                && snapshot
                    .transcript
                    .iter()
                    .any(|message| message.text == "external update")
            {
                break;
            }
        }
    })
    .await
    .expect("background refresh publishes a replacement snapshot");
    assert_eq!(
        reopened.runtime.history().await,
        store.load_messages().unwrap()
    );
    assert_eq!(std::fs::read(store.journal_path()).unwrap(), before);
    let (_, _, deferred, stale) =
        crate::core::PreparedSession::from_dir(store.session_dir().into())
            .unwrap()
            .into_read_model(|_| panic!("background refresh should persist the current view"))
            .unwrap();
    assert!(deferred && !stale);
    reopened.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn clear_persists_empty_view_but_external_append_makes_it_a_resume_candidate() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let config = root.path().join("config.toml");
    let store = saved_startup_store(root.path(), workspace.path(), "before clear").await;
    let handle = AgentAppServer::launch_resumed(
        crate::test_model::config(),
        workspace.path().into(),
        Some(&config),
        store.session_dir().into(),
    )
    .await
    .unwrap();
    handle.clear_history().await.unwrap();
    let (_, view, deferred, stale) =
        crate::core::PreparedSession::from_dir(store.session_dir().into())
            .unwrap()
            .into_read_model(|_| panic!("clear must persist the empty view"))
            .unwrap();
    assert!(deferred && !stale);
    assert!(view.conversation.messages.is_empty());
    assert!(view.transcript.is_empty());
    assert!(
        crate::core::prepare_latest_workspace_session(root.path(), workspace.path())
            .unwrap()
            .is_none()
    );
    handle.shutdown().await;
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(
                MessageRole::User,
                "external after clear",
            )],
        )
        .await
        .unwrap();
    let prepared = crate::core::prepare_latest_workspace_session(root.path(), workspace.path())
        .unwrap()
        .unwrap();
    assert_eq!(prepared.session_dir(), store.session_dir());
    let (_, old, deferred, stale) = prepared
        .into_read_model(|_| panic!("stale view must not block startup"))
        .unwrap();
    assert!(deferred && stale && old.transcript.is_empty());
    let reopened = AgentAppServer::launch_or_resume_latest(
        crate::test_model::config(),
        workspace.path().into(),
        Some(&config),
    )
    .await
    .unwrap();
    assert_eq!(reopened.runtime.session_id(), store.session_id());
    let mut stream = reopened.subscribe_session();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let AppServerEvent::SessionSnapshot { snapshot } = stream.recv().await.unwrap()
                && snapshot
                    .transcript
                    .iter()
                    .any(|message| message.text == "external after clear")
            {
                break;
            }
        }
    })
    .await
    .expect("externally populated session refreshes without a turn");
    reopened.shutdown().await;
}

fn change_startup_metadata(store: &SessionStore, key: &str, version: u32) {
    let path = store.session_dir().join("session.json");
    let mut metadata: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    metadata[key] = serde_json::json!(version);
    std::fs::write(path, serde_json::to_vec(&metadata).unwrap()).unwrap();
}

fn saved_bytes(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            files.extend(saved_bytes(&entry.path()));
        } else {
            files.insert(entry.path(), std::fs::read(entry.path()).unwrap());
        }
    }
    files
}

#[tokio::test]
async fn launch_or_resume_latest_uses_last_non_empty_workspace_session() {
    let cwd = tempfile::tempdir().expect("cwd");
    let config_dir = tempfile::tempdir().expect("config dir");
    let config_path = config_dir.path().join("config.toml");
    let saved_session_id = new_session_id();
    let saved_store =
        SessionStore::new(config_dir.path(), cwd.path(), saved_session_id).expect("session store");
    saved_store
        .append_history(
            crate::domain::new_thread_id(),
            None,
            &[CanonicalMessage::text(
                MessageRole::User,
                "restore saved chat",
            )],
        )
        .await
        .expect("append saved messages");

    let empty_store = SessionStore::new(config_dir.path(), cwd.path(), new_session_id())
        .expect("empty session store");
    let empty_thread = crate::domain::new_thread_id();
    empty_store
        .append_history(
            empty_thread,
            None,
            &[CanonicalMessage::text(MessageRole::User, "temporary")],
        )
        .await
        .expect("materialize empty session");
    empty_store
        .clear_history(empty_thread)
        .await
        .expect("clear empty session");

    let incompatible = saved_startup_store(config_dir.path(), cwd.path(), "newer old chat").await;
    change_startup_metadata(
        &incompatible,
        "journal_schema_version",
        crate::core::JOURNAL_SCHEMA_VERSION - 1,
    );
    // It would be the latest non-empty entry if compatibility filtering failed.
    std::fs::File::open(incompatible.journal_path())
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(60))
        .unwrap();
    let preserved = saved_bytes(incompatible.session_dir());

    let handle = AgentAppServer::launch_or_resume_latest(
        crate::test_model::config(),
        cwd.path().to_path_buf(),
        Some(&config_path),
    )
    .await
    .expect("app server");

    assert_eq!(
        handle.runtime.session_dir(),
        Some(saved_store.session_dir())
    );
    assert_eq!(handle.runtime.history().await.len(), 1);
    assert_eq!(
        handle.transcript().await.expect("transcript")[0].text,
        "restore saved chat".to_owned()
    );
    assert_eq!(saved_bytes(incompatible.session_dir()), preserved);
    handle.shutdown().await;
}

#[tokio::test]
async fn launch_or_resume_latest_starts_fresh_when_all_persisted_sessions_are_unusable() {
    let cwd = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let config_path = root.path().join("config.toml");
    let old_metadata = saved_startup_store(root.path(), cwd.path(), "old metadata").await;
    change_startup_metadata(&old_metadata, "schema_version", 3);
    let old_journal_tag = saved_startup_store(root.path(), cwd.path(), "old journal tag").await;
    change_startup_metadata(
        &old_journal_tag,
        "journal_schema_version",
        crate::core::JOURNAL_SCHEMA_VERSION - 1,
    );
    let old_journal = saved_startup_store(root.path(), cwd.path(), "old journal").await;
    std::fs::write(
        old_journal.journal_path(),
        format!(
            "{{\"schema_version\":{},\"old_shape\":true}}\n",
            crate::core::JOURNAL_SCHEMA_VERSION - 1
        ),
    )
    .unwrap();
    let uuid_dir = old_metadata
        .session_dir()
        .parent()
        .unwrap()
        .join(new_session_id().to_string());
    std::fs::create_dir(&uuid_dir).unwrap();
    std::fs::write(uuid_dir.join("messages.jsonl"), b"old uuid history\n").unwrap();
    let before = saved_bytes(&root.path().join("sessions"));
    assert!(SessionStore::open(old_metadata.session_dir().to_path_buf()).is_err());
    assert!(
        AgentAppServer::launch_resumed(
            crate::test_model::config(),
            cwd.path().to_path_buf(),
            Some(&config_path),
            old_metadata.session_dir().to_path_buf()
        )
        .await
        .is_err()
    );

    let handle = AgentAppServer::launch_or_resume_latest(
        crate::test_model::config(),
        cwd.path().to_path_buf(),
        Some(&config_path),
    )
    .await
    .unwrap();
    assert!(handle.runtime.history().await.is_empty());
    assert!(handle.transcript().await.unwrap().is_empty());
    assert!(handle.workspace_session_summaries().unwrap().is_empty());
    assert_ne!(handle.session_id(), old_metadata.session_id());
    assert_eq!(saved_bytes(&root.path().join("sessions")), before);
    // Persisting the fresh chat must work alongside the hidden sessions.
    handle
        .send_user_message("fresh chat".to_owned())
        .await
        .unwrap();
    let summaries = handle.workspace_session_summaries().unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].session_id, handle.session_id());
    for dir in [
        old_metadata.session_dir(),
        old_journal_tag.session_dir(),
        old_journal.session_dir(),
        uuid_dir.as_path(),
    ] {
        let expected: BTreeMap<_, _> = before
            .iter()
            .filter(|(path, _)| path.starts_with(dir))
            .map(|(path, bytes)| (path.clone(), bytes.clone()))
            .collect();
        assert_eq!(saved_bytes(dir), expected);
    }
    handle.shutdown().await;
}

#[tokio::test]
async fn launch_or_resume_latest_does_not_hide_storage_root_errors() {
    let cwd = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("sessions"), b"not a directory").unwrap();
    assert!(
        AgentAppServer::launch_or_resume_latest(
            crate::test_model::config(),
            cwd.path().to_path_buf(),
            Some(&root.path().join("config.toml"))
        )
        .await
        .is_err()
    );
}
