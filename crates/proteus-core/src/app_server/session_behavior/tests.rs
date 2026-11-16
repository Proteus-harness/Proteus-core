use super::*;
use crate::{
    app_server::AgentAppServer,
    domain::{BehaviorModeDefinition, ModelRef, ReasoningConfig, ToolCall},
};
use proteus_contracts::app_protocol::{
    http::SetConfigBuilderRequest,
    settings::{SessionSettingControl, SessionSettingUpdate},
};
use serde_json::json;
use std::path::Path;

fn config() -> AppConfig {
    let mut config = crate::test_model::config();
    crate::test_model::add_direct_patch_tool(&mut config);
    config.tools.enabled = vec!["request_user_input".into(), "apply_patch".into()];
    config.modes.insert(
        "quiet".into(),
        BehaviorModeDefinition {
            title: Some("Quiet".into()),
            instructions: Some("First mode instructions".into()),
            tools: Some(vec![]),
            hooks: Some(vec![]),
            permission_mode: Some(PermissionMode::Plan),
        },
    );
    config
        .modes
        .insert("inherit".into(), BehaviorModeDefinition::default());
    config
}

async fn launch(path: &Path, workspace: &Path) -> AppServerHandle {
    AgentAppServer::launch(
        AppConfig::load(Some(path)).await.unwrap(),
        workspace.into(),
        Some(path),
    )
    .await
    .unwrap()
}

async fn setup(root: &Path, extension: &str) -> (std::path::PathBuf, AppServerHandle) {
    let path = root.join(format!("agent.{extension}"));
    let config = config();
    let text = if extension == "json" {
        serde_json::to_string_pretty(&config).unwrap()
    } else {
        format!(
            "# keep source comment\n{}",
            toml::to_string_pretty(&config).unwrap().replace(
                "First mode instructions\"",
                "First mode instructions\" # keep instruction comment"
            )
        )
    };
    std::fs::write(&path, text).unwrap();
    let server = launch(&path, root).await;
    (path, server)
}

async fn select(server: &AppServerHandle, id: &str) -> Result<()> {
    server
        .set_session_setting(SessionSettingUpdate {
            id: "behavior_mode".into(),
            value: json!(id),
        })
        .await
}

async fn selected(server: &AppServerHandle) -> String {
    let setting = server
        .session_settings()
        .await
        .into_iter()
        .find(|setting| setting.id == "behavior_mode")
        .unwrap();
    let SessionSettingControl::Select { value, .. } = setting.control else {
        panic!()
    };
    value
}

#[tokio::test]
async fn behavior_is_per_chat_and_empty_tools_removes_callable_and_nested_surface() {
    let root = tempfile::tempdir().unwrap();
    let (path, server) = setup(root.path(), "json").await;
    let other = launch(&path, root.path()).await;
    let source = std::fs::read(&path).unwrap();
    assert!(!server.runtime.tool_entries().await.is_empty());
    select(&server, "quiet").await.unwrap();
    assert_eq!(selected(&server).await, "quiet");
    assert_eq!(selected(&other).await, DEFAULT);
    assert_eq!(server.permission_mode().await, PermissionMode::Plan);
    assert_eq!(other.permission_mode().await, PermissionMode::Normal);
    assert!(server.runtime.tool_entries().await.is_empty());
    assert!(!other.runtime.tool_entries().await.is_empty());
    let (snapshot, _, _, _) = server.runtime.configuration_view().await;
    assert!(
        snapshot
            .registry
            .tools_for_model(
                snapshot
                    .registry
                    .model_config
                    .as_ref()
                    .map(|m| m.model_ref())
                    .as_ref()
            )
            .unwrap()
            .specs()
            .is_empty()
    );
    let result = server
        .run_tool_call(
            None,
            ToolCall::new("denied", "request_user_input", json!({})),
        )
        .await
        .unwrap();
    assert!(!result.ok);
    assert!(
        snapshot
            .registry
            .instructions
            .iter()
            .any(|block| block.text == "First mode instructions")
    );
    assert!(snapshot.registry.hooks.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), source);
    let editor = server.config_builder_snapshot().await;
    assert_eq!(editor.tools_enabled, ["request_user_input", "apply_patch"]);
    for name in ["request_user_input", "apply_patch"] {
        let tool = editor.tools.iter().find(|tool| tool.name == name).unwrap();
        assert!(
            tool.enabled && tool.registered,
            "saved baseline {name} remains available in editor"
        );
    }
    select(&server, DEFAULT).await.unwrap();
    assert!(!server.runtime.tool_entries().await.is_empty());
    assert_eq!(server.permission_mode().await, PermissionMode::Normal);
    server.shutdown().await;
    other.shutdown().await;
}

#[tokio::test]
async fn model_and_behavior_overrides_compose_in_both_orders_and_survive_resume() {
    let root = tempfile::tempdir().unwrap();
    let (path, server) = setup(root.path(), "json").await;
    server.set_model_name("ordinary".into()).await.unwrap();
    select(&server, "quiet").await.unwrap();
    assert_eq!(server.runtime.model_ref().await.unwrap().model, "ordinary");
    server
        .apply_model_configuration(
            ModelRef::new("fake", "exact"),
            ReasoningConfig::default(),
            false,
        )
        .await
        .unwrap();
    assert!(server.runtime.tool_entries().await.is_empty());
    assert_eq!(selected(&server).await, "quiet");
    select(&server, "inherit").await.unwrap();
    assert_eq!(server.runtime.model_ref().await.unwrap().model, "exact");
    assert!(!server.runtime.tool_entries().await.is_empty());
    select(&server, "quiet").await.unwrap();
    server
        .apply_model_configuration(
            ModelRef::new("fake", "fast"),
            ReasoningConfig::default(),
            true,
        )
        .await
        .unwrap();
    assert!(server.runtime.tool_entries().await.is_empty());
    server
        .set_permission_mode(PermissionMode::Auto)
        .await
        .unwrap();
    let session = server.session_dir_path().unwrap();
    server.shutdown().await;
    drop(server);
    let resumed = AgentAppServer::launch_resumed(
        AppConfig::load(Some(&path)).await.unwrap(),
        root.path().into(),
        Some(&path),
        session.clone(),
    )
    .await
    .unwrap();
    assert_eq!(selected(&resumed).await, "quiet");
    assert_eq!(resumed.runtime.model_ref().await.unwrap().model, "fast");
    assert_eq!(resumed.permission_mode().await, PermissionMode::Auto);
    assert!(resumed.runtime.tool_entries().await.is_empty());
    let fresh = launch(&path, root.path()).await;
    assert_eq!(selected(&fresh).await, DEFAULT);
    assert_eq!(fresh.permission_mode().await, PermissionMode::Normal);
    select(&resumed, "quiet").await.unwrap();
    assert_eq!(
        resumed.permission_mode().await,
        PermissionMode::Plan,
        "new behavior selection clears explicit permission override"
    );
    select(&resumed, DEFAULT).await.unwrap();
    resumed.shutdown().await;
    drop(resumed);
    let resumed = AgentAppServer::launch_resumed(
        AppConfig::load(Some(&path)).await.unwrap(),
        root.path().into(),
        Some(&path),
        session,
    )
    .await
    .unwrap();
    assert_eq!(selected(&resumed).await, DEFAULT);
    assert!(!resumed.runtime.tool_entries().await.is_empty());
    resumed.shutdown().await;
    fresh.shutdown().await;
}

#[tokio::test]
async fn edited_selected_definitions_reapply_atomically_and_deletion_restores_baseline() {
    for extension in ["toml", "json"] {
        let root = tempfile::tempdir().unwrap();
        let (path, server) = setup(root.path(), extension).await;
        select(&server, "quiet").await.unwrap();
        let old = server.config_builder_snapshot().await;
        assert!(old.modes_writable);
        let mut modes = old.modes.clone();
        modes.get_mut("quiet").unwrap().instructions = Some("New instructions".into());
        server
            .set_profile_config(SetConfigBuilderRequest {
                modes: Some(modes.clone()),
                expected_modes: Some(old.modes.clone()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(selected(&server).await, "quiet");
        let (runtime, _, _, _) = server.runtime.configuration_view().await;
        assert!(
            runtime
                .registry
                .instructions
                .iter()
                .any(|block| block.text == "New instructions")
        );
        assert!(
            !runtime
                .registry
                .instructions
                .iter()
                .any(|block| block.text == "First mode instructions")
        );
        let source = std::fs::read(&path).unwrap();
        if extension == "toml" {
            let text = String::from_utf8(source.clone()).unwrap();
            assert!(text.contains("# keep source comment"));
            assert!(text.contains("# keep instruction comment"));
        }
        let mut bad = modes.clone();
        bad.get_mut("quiet").unwrap().hooks = Some(vec!["missing-hook".into()]);
        let epoch = server.runtime.module_epoch().await;
        assert!(
            server
                .set_profile_config(SetConfigBuilderRequest {
                    modes: Some(bad),
                    ..Default::default()
                })
                .await
                .is_err()
        );
        assert_eq!(server.runtime.module_epoch().await, epoch);
        assert_eq!(std::fs::read(&path).unwrap(), source);
        assert_eq!(server.config_builder_snapshot().await.modes, modes);
        assert!(
            server
                .set_profile_config(SetConfigBuilderRequest {
                    modes: Some(modes.clone()),
                    expected_modes: Some(old.modes.clone()),
                    ..Default::default()
                })
                .await
                .is_err()
        );
        let history = server.config_history().await.unwrap();
        assert_eq!(history.revisions[0].state.modes, old.modes);
        server
            .set_profile_config(SetConfigBuilderRequest {
                modes: Some(history.revisions[0].state.modes.clone()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(selected(&server).await, "quiet");
        modes.remove("quiet");
        server
            .set_profile_config(SetConfigBuilderRequest {
                modes: Some(modes),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(selected(&server).await, DEFAULT);
        assert!(!server.runtime.tool_entries().await.is_empty());
        assert_eq!(server.permission_mode().await, PermissionMode::Normal);
        server
            .set_profile_config(SetConfigBuilderRequest {
                modes: Some(old.modes.clone()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            selected(&server).await,
            DEFAULT,
            "restoring a deleted definition does not select it"
        );
        let session = server.session_dir_path().unwrap();
        server.shutdown().await;
        drop(server);
        let resumed = AgentAppServer::launch_resumed(
            AppConfig::load(Some(&path)).await.unwrap(),
            root.path().into(),
            Some(&path),
            session,
        )
        .await
        .unwrap();
        assert_eq!(selected(&resumed).await, DEFAULT);
        resumed.shutdown().await;
    }
}

#[tokio::test]
async fn inherited_modes_are_readonly_and_failed_metadata_commit_keeps_effective_state() {
    let root = tempfile::tempdir().unwrap();
    let (base, server) = setup(root.path(), "json").await;
    let wrapper = root.path().join("wrapper.json");
    std::fs::write(
        &wrapper,
        serde_json::to_vec(&json!({"include":base})).unwrap(),
    )
    .unwrap();
    let inherited = launch(&wrapper, root.path()).await;
    let snapshot = inherited.config_builder_snapshot().await;
    assert!(!snapshot.modes_writable);
    let mut modes = snapshot.modes.clone();
    modes.remove("quiet");
    assert!(
        inherited
            .set_profile_config(SetConfigBuilderRequest {
                modes: Some(modes),
                ..Default::default()
            })
            .await
            .is_err()
    );
    let before = server.runtime.module_epoch().await;
    let selection = server.session_dir_path().unwrap().join(FILE);
    std::fs::create_dir_all(&selection).unwrap();
    assert!(select(&server, "quiet").await.is_err());
    assert_eq!(server.runtime.module_epoch().await, before);
    assert_eq!(selected(&server).await, DEFAULT);
    assert_eq!(server.permission_mode().await, PermissionMode::Normal);
    assert!(
        server
            .set_permission_mode(PermissionMode::Auto)
            .await
            .is_err()
    );
    assert_eq!(server.permission_mode().await, PermissionMode::Normal);
    server.shutdown().await;
    inherited.shutdown().await;
}

#[tokio::test]
async fn behavior_switch_rejects_active_runs_and_malformed_definitions_without_writes() {
    let root = tempfile::tempdir().unwrap();
    let (path, server) = setup(root.path(), "json").await;
    server
        .register_test_run("pending-run", crate::contracts::CancellationToken::new())
        .await;
    let source = std::fs::read(&path).unwrap();
    let epoch = server.runtime.module_epoch().await;
    let error = select(&server, "quiet").await.unwrap_err();
    assert!(error.to_string().contains("idle"));
    assert_eq!(server.runtime.module_epoch().await, epoch);
    assert_eq!(std::fs::read(&path).unwrap(), source);
    let mut modes = server.config_builder_snapshot().await.modes;
    modes.insert("_default".into(), BehaviorModeDefinition::default());
    assert!(
        server
            .set_profile_config(SetConfigBuilderRequest {
                modes: Some(modes),
                ..Default::default()
            })
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), source);
    server.shutdown().await;
}

#[tokio::test]
async fn deleted_selection_metadata_rolls_back_on_source_failure_and_blocks_save_on_metadata_failure()
 {
    let root = tempfile::tempdir().unwrap();
    let (path, server) = setup(root.path(), "json").await;
    select(&server, "quiet").await.unwrap();
    let metadata = server.session_dir_path().unwrap().join(FILE);
    let before = std::fs::read(&metadata).unwrap();
    let source = std::fs::read(&path).unwrap();
    let epoch = server.runtime.module_epoch().await;
    let mut config = server.config.read().await.clone();
    config.modes.remove("quiet");
    let assembly = prepare_assembly(&config, root.path(), Some(&path))
        .await
        .unwrap();
    let error = server
        .publish_profile(config.clone(), assembly, None, || async {
            anyhow::bail!("synthetic source failure")
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("synthetic source failure"));
    assert_eq!(std::fs::read(&metadata).unwrap(), before);
    assert_eq!(std::fs::read(&path).unwrap(), source);
    assert_eq!(selected(&server).await, "quiet");
    assert_eq!(server.runtime.module_epoch().await, epoch);
    std::fs::remove_file(&metadata).unwrap();
    std::fs::create_dir(&metadata).unwrap();
    assert!(
        server
            .set_profile_config(SetConfigBuilderRequest {
                modes: Some(config.modes),
                ..Default::default()
            })
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), source);
    assert_eq!(server.runtime.module_epoch().await, epoch);
    assert_eq!(selected(&server).await, "quiet");
    server.shutdown().await;
}

#[tokio::test]
async fn deletion_while_closed_is_durable_before_a_definition_is_restored() {
    let root = tempfile::tempdir().unwrap();
    let (path, server) = setup(root.path(), "json").await;
    let original = server.config.read().await.clone();
    select(&server, "quiet").await.unwrap();
    let session = server.session_dir_path().unwrap();
    server.shutdown().await;
    drop(server);
    let mut removed = original.clone();
    removed.modes.remove("quiet");
    std::fs::write(&path, serde_json::to_vec_pretty(&removed).unwrap()).unwrap();
    let resumed = AgentAppServer::launch_resumed(
        AppConfig::load(Some(&path)).await.unwrap(),
        root.path().into(),
        Some(&path),
        session.clone(),
    )
    .await
    .unwrap();
    assert_eq!(selected(&resumed).await, DEFAULT);
    resumed
        .set_profile_config(SetConfigBuilderRequest {
            modes: Some(original.modes),
            ..Default::default()
        })
        .await
        .unwrap();
    resumed.shutdown().await;
    drop(resumed);
    let resumed = AgentAppServer::launch_resumed(
        AppConfig::load(Some(&path)).await.unwrap(),
        root.path().into(),
        Some(&path),
        session,
    )
    .await
    .unwrap();
    assert_eq!(selected(&resumed).await, DEFAULT);
    assert!(!resumed.runtime.tool_entries().await.is_empty());
    resumed.shutdown().await;
}
