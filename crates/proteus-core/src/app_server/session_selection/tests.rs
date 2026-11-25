use super::*;
use crate::app_server::AgentAppServer;

#[tokio::test]
async fn inline_roles_keep_separate_last_models_inside_one_physical_profile_store() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let source = root.path().join("agent.json");
    let baseline = crate::test_model::config();
    let mut document = serde_json::to_value(&baseline).unwrap();
    document["agent_control"]["roles"] = serde_json::json!([
        { "name": "explore/read", "description": "Reader", "config": baseline },
        { "name": "coder", "description": "Writer", "config": baseline }
    ]);
    std::fs::write(&source, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let reader_path = crate::core::config_reference(&source, "explore/read").unwrap();
    let coder_path = crate::core::config_reference(&source, "coder").unwrap();
    let reader_config = AppConfig::load(Some(&reader_path)).await.unwrap();
    let coder_config = AppConfig::load(Some(&coder_path)).await.unwrap();
    assert_eq!(reader_config.profile.name, coder_config.profile.name);
    assert_ne!(
        last_path(&reader_config, &reader_path).unwrap(),
        last_path(&coder_config, &coder_path).unwrap()
    );
    assert_eq!(config_store_root(&reader_path), root.path());
    let reader =
        AgentAppServer::launch(reader_config.clone(), cwd.path().into(), Some(&reader_path))
            .await
            .unwrap();
    reader
        .set_model_name("reader-only-choice".into())
        .await
        .unwrap();
    reader.shutdown().await;
    drop(reader);
    let coder = AgentAppServer::launch(coder_config, cwd.path().into(), Some(&coder_path))
        .await
        .unwrap();
    assert_eq!(
        coder.runtime.model_ref().await.unwrap().model,
        baseline.active_model_config().unwrap().model
    );
    coder
        .set_model_name("coder-only-choice".into())
        .await
        .unwrap();
    coder.shutdown().await;
    drop(coder);
    let reader = AgentAppServer::launch(reader_config, cwd.path().into(), Some(&reader_path))
        .await
        .unwrap();
    assert_eq!(
        reader.runtime.model_ref().await.unwrap().model,
        "reader-only-choice"
    );
    reader.shutdown().await;
}

async fn launch(root: &Path, cwd: &Path) -> super::super::AppServerHandle {
    AgentAppServer::launch(
        crate::test_model::config(),
        cwd.into(),
        Some(&root.join("config.toml")),
    )
    .await
    .unwrap()
}

async fn resume(root: &Path, cwd: &Path, dir: PathBuf) -> super::super::AppServerHandle {
    AgentAppServer::launch_resumed(
        crate::test_model::config(),
        cwd.into(),
        Some(&root.join("config.toml")),
        dir,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn unsent_model_and_reasoning_survive_handle_recreation_without_journal_writes() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let first = launch(root.path(), cwd.path()).await;
    first.set_model_name("remember-this".into()).await.unwrap();
    first
        .set_reasoning_effort(Some("high".into()))
        .await
        .unwrap();
    let reasoning = first.runtime.reasoning().await;
    let dir = first.session_dir_path().unwrap();
    assert!(
        !first
            .runtime
            .session_store()
            .unwrap()
            .journal_path()
            .exists()
    );
    first.shutdown().await;
    drop(first);
    let restored = resume(root.path(), cwd.path(), dir).await;
    assert_eq!(
        restored.runtime.model_ref().await.unwrap().model,
        "remember-this"
    );
    assert_eq!(restored.runtime.reasoning().await, reasoning);
    restored.set_reasoning_enabled(false).await.unwrap();
    let dir = restored.session_dir_path().unwrap();
    restored.shutdown().await;
    drop(restored);
    let disabled = resume(root.path(), cwd.path(), dir).await;
    assert!(!disabled.runtime.reasoning().await.is_enabled());
    disabled.shutdown().await;
}

#[tokio::test]
async fn existing_chats_are_independent_and_only_new_chat_inherits_last_choice() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let first = launch(root.path(), cwd.path()).await;
    first.set_model_name("first-model".into()).await.unwrap();
    let first_dir = first.session_dir_path().unwrap();
    let second = launch(root.path(), cwd.path()).await;
    assert_eq!(
        second.runtime.model_ref().await.unwrap().model,
        "first-model"
    );
    let second_dir = second.session_dir_path().unwrap();
    second.set_model_name("second-model".into()).await.unwrap();
    first.shutdown().await;
    second.shutdown().await;
    drop(first);
    drop(second);
    let restored_first = resume(root.path(), cwd.path(), first_dir).await;
    let restored_second = resume(root.path(), cwd.path(), second_dir).await;
    let third = launch(root.path(), cwd.path()).await;
    assert_eq!(
        restored_first.runtime.model_ref().await.unwrap().model,
        "first-model"
    );
    assert_eq!(
        restored_second.runtime.model_ref().await.unwrap().model,
        "second-model"
    );
    assert_eq!(
        third.runtime.model_ref().await.unwrap().model,
        "second-model"
    );
    restored_first.shutdown().await;
    restored_second.shutdown().await;
    third.shutdown().await;
}

#[tokio::test]
async fn existing_chat_without_sidecar_uses_profile_baseline_not_global_last() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let old = launch(root.path(), cwd.path()).await;
    old.runtime
        .session_store()
        .unwrap()
        .write_metadata(
            "test-materialized.json",
            serde_json::json!({"materialized": true}),
        )
        .await
        .unwrap();
    let old_dir = old.session_dir_path().unwrap();
    old.shutdown().await;
    drop(old);
    let changed = launch(root.path(), cwd.path()).await;
    changed.set_model_name("new-default".into()).await.unwrap();
    let restored = resume(root.path(), cwd.path(), old_dir).await;
    assert_eq!(
        restored.runtime.model_ref().await.unwrap().model,
        crate::test_model::config()
            .active_model_config()
            .unwrap()
            .model
    );
    changed.shutdown().await;
    restored.shutdown().await;
}

#[tokio::test]
async fn failed_global_write_rolls_back_session_and_does_not_publish_runtime() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let handle = launch(root.path(), cwd.path()).await;
    handle.set_model_name("before".into()).await.unwrap();
    let before: serde_json::Value = read_session(handle.runtime.session_store().unwrap(), FILE)
        .await
        .unwrap()
        .unwrap();
    let last = last_path(
        &crate::test_model::config(),
        &root.path().join("config.toml"),
    )
    .unwrap();
    std::fs::remove_file(&last).unwrap();
    std::fs::create_dir(&last).unwrap();
    assert!(handle.set_model_name("must-fail".into()).await.is_err());
    assert_eq!(handle.runtime.model_ref().await.unwrap().model, "before");
    assert_eq!(
        read_session::<serde_json::Value>(handle.runtime.session_store().unwrap(), FILE)
            .await
            .unwrap()
            .unwrap(),
        before
    );
    handle.shutdown().await;
}

#[tokio::test]
async fn selection_metadata_never_hides_corrupt_canonical_history() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let handle = launch(root.path(), cwd.path()).await;
    handle.set_model_name("saved".into()).await.unwrap();
    let dir = handle.session_dir_path().unwrap();
    let journal = handle.runtime.session_store().unwrap().journal_path();
    handle.shutdown().await;
    drop(handle);
    std::fs::write(journal, b"invalid journal record\n").unwrap();
    let result = AgentAppServer::launch_resumed(
        crate::test_model::config(),
        cwd.path().into(),
        Some(&root.path().join("config.toml")),
        dir,
    )
    .await;
    assert!(result.is_err());
}

#[test]
fn profile_provider_and_adapter_route_bind_choices_without_growing_route_files() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.toml");
    let config = AppConfig::default();
    let original = binding(&config, &path).unwrap();
    let mut other = config.clone();
    other
        .providers
        .insert("other".into(), config.providers["fake"].clone());
    other.active_provider = Some("other".into());
    assert_ne!(binding(&other, &path).unwrap(), original);
    assert_ne!(
        last_path(&other, &path).unwrap(),
        last_path(&config, &path).unwrap()
    );
    assert_ne!(
        binding(&config, &root.path().join("another.toml")).unwrap(),
        original
    );
    other = config.clone();
    other.providers.get_mut("fake").unwrap().model = "changed-profile-default".into();
    assert_ne!(binding(&other, &path).unwrap(), original);
    assert_eq!(
        last_path(&other, &path).unwrap(),
        last_path(&config, &path).unwrap()
    );
    other = config.clone();
    other
        .module_config
        .entry("model".into())
        .or_default()
        .insert(
            "fake".into(),
            serde_json::json!({"endpoint":"another-route"}),
        );
    assert_ne!(binding(&other, &path).unwrap(), original);
    assert_eq!(
        last_path(&other, &path).unwrap(),
        last_path(&config, &path).unwrap()
    );
}

#[tokio::test]
async fn route_reload_and_cold_resume_agree_on_global_model_and_reasoning() {
    let mut mismatches = Vec::new();
    for change in ["route", "component"] {
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        let mut config = crate::test_model::config();
        config.providers.get_mut("fake").unwrap().reasoning =
            ReasoningConfig::new(Some("low".into()), true);
        let server = AgentAppServer::launch(config.clone(), cwd.path().into(), Some(&path))
            .await
            .unwrap();
        server.set_model_name("ui-choice".into()).await.unwrap();
        server
            .set_reasoning_effort(Some("high".into()))
            .await
            .unwrap();
        let ui_model = server.runtime.model_ref().await;
        let ui_reasoning = server.runtime.reasoning().await;
        let directory = server.session_dir_path().unwrap();
        let saved: serde_json::Value = read_session(server.runtime.session_store().unwrap(), FILE)
            .await
            .unwrap()
            .unwrap();

        // An unrelated refresh must not discard the user's selection.
        config.runtime.context_timeout_ms += 1;
        let assembly =
            super::super::prepare_assembly(config.clone().into(), cwd.path(), Some(&path))
                .await
                .unwrap();
        server
            .publish_profile(assembly, None, || async { Ok(()) })
            .await
            .unwrap();
        assert_eq!(server.runtime.model_ref().await, ui_model);
        assert_eq!(server.runtime.reasoning().await, ui_reasoning);

        match change {
            "route" => {
                config
                    .module_config
                    .get_mut("model")
                    .unwrap()
                    .get_mut("fake")
                    .unwrap()["stream_delay_ms"] = 2.into();
            }
            "component" => {
                let component = config.components.get_mut("test-model").unwrap();
                let mut value = serde_json::to_value(&*component).unwrap();
                value["handshake_timeout_ms"] = 30_001.into();
                *component = serde_json::from_value(value).unwrap();
            }
            _ => unreachable!(),
        }
        let configured = config.selected_model_config().unwrap().unwrap();
        let assembly =
            super::super::prepare_assembly(config.clone().into(), cwd.path(), Some(&path))
                .await
                .unwrap();
        server
            .publish_profile(assembly, None, || async { Ok(()) })
            .await
            .unwrap();
        let hot_model = server.runtime.model_ref().await;
        let hot_reasoning = server.runtime.reasoning().await;
        assert_eq!(
            read_session::<serde_json::Value>(server.runtime.session_store().unwrap(), FILE)
                .await
                .unwrap()
                .unwrap(),
            saved
        );
        server.shutdown().await;
        drop(server);

        let cold =
            AgentAppServer::launch_resumed(config, cwd.path().into(), Some(&path), directory)
                .await
                .unwrap();
        let cold_model = cold.runtime.model_ref().await;
        let cold_reasoning = cold.runtime.reasoning().await;
        assert_eq!(cold_model, Some(configured.model_ref()));
        assert_eq!(cold_reasoning, configured.reasoning);
        if hot_model != cold_model || hot_reasoning != cold_reasoning {
            mismatches.push((change, hot_model, hot_reasoning, cold_model, cold_reasoning));
        }
        cold.shutdown().await;
    }
    assert!(mismatches.is_empty(), "hot/cold mismatch: {mismatches:?}");
}

#[tokio::test]
async fn profile_without_a_persistent_path_keeps_its_live_selection_on_route_reload() {
    let cwd = tempfile::tempdir().unwrap();
    let mut config = crate::test_model::config();
    let server = AgentAppServer::launch(config.clone(), cwd.path().into(), None)
        .await
        .unwrap();
    server.set_model_name("ui-choice".into()).await.unwrap();
    server
        .set_reasoning_effort(Some("high".into()))
        .await
        .unwrap();
    let model = server.runtime.model_ref().await;
    let reasoning = server.runtime.reasoning().await;
    config
        .module_config
        .get_mut("model")
        .unwrap()
        .get_mut("fake")
        .unwrap()["stream_delay_ms"] = 2.into();
    let assembly = super::super::prepare_assembly(config.into(), cwd.path(), None)
        .await
        .unwrap();
    server
        .publish_profile(assembly, None, || async { Ok(()) })
        .await
        .unwrap();
    assert_eq!(server.runtime.model_ref().await, model);
    assert_eq!(server.runtime.reasoning().await, reasoning);
    server.shutdown().await;
}

#[test]
fn oversized_and_invalid_metadata_are_explicit_errors_and_wrong_binding_is_ignored() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(FILE);
    std::fs::write(&path, vec![b' '; MAX_BYTES as usize + 1]).unwrap();
    assert!(read_selection(&path, "expected").is_err());
    std::fs::write(&path, b"{}").unwrap();
    assert!(read_selection(&path, "expected").is_err());
    let selection = Selection {
        version: 1,
        binding: "old-provider".into(),
        model: ModelRef::new("fake", "a"),
        reasoning: ReasoningConfig::default(),
    };
    std::fs::write(&path, serde_json::to_vec(&selection).unwrap()).unwrap();
    assert!(read_selection(&path, "expected").unwrap().is_none());
}

struct PersistencePause {
    entered: tokio::sync::oneshot::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

fn pauses() -> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, PersistencePause>> {
    static PAUSES: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, PersistencePause>>,
    > = std::sync::OnceLock::new();
    PAUSES.get_or_init(Default::default)
}

fn pause_next(
    path: PathBuf,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    let (entered, wait) = tokio::sync::oneshot::channel();
    let (release, receiver) = std::sync::mpsc::channel();
    assert!(
        pauses()
            .lock()
            .unwrap()
            .insert(
                path,
                PersistencePause {
                    entered,
                    release: receiver
                }
            )
            .is_none()
    );
    (wait, release)
}

pub(super) fn pause_persistence(path: &Path) {
    let pause = pauses().lock().unwrap().remove(path);
    if let Some(pause) = pause {
        let _ = pause.entered.send(());
        let _ = pause
            .release
            .recv_timeout(std::time::Duration::from_secs(10));
    }
}

#[tokio::test]
async fn cancelled_request_keeps_transaction_owned_until_publish_before_next_selection() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let handle = launch(root.path(), cwd.path()).await;
    handle.set_model_name("initial".into()).await.unwrap();
    let session = handle.session_dir_path().unwrap().join(FILE);
    let (entered, release) = pause_next(session.clone());
    let first_handle = handle.clone();
    let first = tokio::spawn(async move {
        first_handle
            .set_model_name("cancelled-request-choice".into())
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), entered)
        .await
        .unwrap()
        .unwrap();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let (started, wait_started) = tokio::sync::oneshot::channel();
    let second_handle = handle.clone();
    let second = tokio::spawn(async move {
        let _ = started.send(());
        second_handle.set_model_name("latest-choice".into()).await
    });
    wait_started.await.unwrap();
    // A holds the real transaction, not its cancelled request's await frame.
    assert_eq!(handle.runtime.model_ref().await.unwrap().model, "initial");
    release.send(()).unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(
        handle.runtime.model_ref().await.unwrap().model,
        "latest-choice"
    );
    let stored: Selection = read_session(handle.runtime.session_store().unwrap(), FILE)
        .await
        .unwrap()
        .unwrap();
    let last = last_path(
        &crate::test_model::config(),
        &root.path().join("config.toml"),
    )
    .unwrap();
    let global: Selection = serde_json::from_slice(&std::fs::read(last).unwrap()).unwrap();
    assert_eq!(stored.model.model, "latest-choice");
    assert_eq!(global.model, stored.model);
    handle.shutdown().await;
}

#[tokio::test]
async fn concurrent_delete_waits_for_metadata_write_and_never_resurrects_session() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let handle = launch(root.path(), cwd.path()).await;
    handle.set_model_name("initial".into()).await.unwrap();
    let directory = handle.session_dir_path().unwrap();
    let (entered, release) = pause_next(directory.join(FILE));
    let selecting_handle = handle.clone();
    let selecting = tokio::spawn(async move {
        selecting_handle
            .set_model_name("during-delete".into())
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), entered)
        .await
        .unwrap()
        .unwrap();
    let (started, wait_started) = tokio::sync::oneshot::channel();
    let deleting_handle = handle.clone();
    let deleting_directory = directory.clone();
    let deleting = tokio::spawn(async move {
        let _ = started.send(());
        deleting_handle
            .delete_workspace_session(deleting_directory)
            .await
    });
    wait_started.await.unwrap();
    assert!(directory.exists());
    release.send(()).unwrap();
    selecting.await.unwrap().unwrap();
    assert!(deleting.await.unwrap().unwrap());
    assert!(!directory.exists());
    // A subsequent setter sees retired ownership and cannot create the dir.
    assert!(handle.set_model_name("after-delete".into()).await.is_err());
    assert!(!directory.exists());
    handle.shutdown().await;
}

#[test]
fn component_icon_does_not_invalidate_saved_model_selection_binding() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.toml");
    let mut config = crate::test_model::config();
    let before = binding(&config, &path).unwrap();
    let component = config.components.get_mut("test-model").unwrap();
    let mut source = serde_json::to_value(&*component).unwrap();
    source["icon"] = serde_json::json!("assets/model.svg");
    *component = serde_json::from_value(source).unwrap();
    assert_eq!(binding(&config, &path).unwrap(), before);
}
