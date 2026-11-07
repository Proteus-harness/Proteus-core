use super::super::AgentAppServer;
use super::*;

#[tokio::test]
async fn directory_save_has_explicit_precedence_over_late_fragments() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(dir.path().join("z-settings.toml"), "[permissions]\nmode='normal'\n[modules]\nworkflow='old'\n[module_config.workflow.old]\nvalue=1\n[tools]\nenabled=['old']\n").await.unwrap();
    let mut config = AppConfig::default();
    config.permissions.mode = PermissionMode::Plan;
    config.tools.enabled = vec!["new".into()];
    let target = config_builder_target_path(Some(dir.path())).unwrap();
    persist_config_builder(&target, &config).await.unwrap();
    let loaded = AppConfig::load(Some(dir.path())).await.unwrap();
    assert_eq!(loaded.permissions.mode, PermissionMode::Plan);
    assert_eq!(loaded.tools.enabled, config.tools.enabled);
    assert_eq!(loaded.modules.workflow, None);
    assert!(loaded.module_config.is_empty());
}

#[tokio::test]
async fn concurrent_saves_publish_the_same_profile_as_disk() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("config.toml");
    let initial_config = crate::test_model::config();
    let server = AgentAppServer::launch(initial_config.clone(), home.path().into(), Some(&source))
        .await
        .unwrap();
    let snapshot = server.config_builder_snapshot().await;
    let schema = snapshot.model_modules[0]
        .config_schema
        .as_ref()
        .expect("real process model schema");
    assert!(
        schema
            .fields
            .iter()
            .any(|field| field.key == "implementation")
    );
    assert_eq!(
        snapshot.module_config, initial_config.module_config,
        "describing defaults must not materialize configuration"
    );
    let mut tasks = Vec::new();
    for mode in [
        PermissionMode::Plan,
        PermissionMode::Auto,
        PermissionMode::Normal,
        PermissionMode::Plan,
    ] {
        let server = server.clone();
        tasks.push(tokio::spawn(async move {
            server
                .set_config_builder(
                    BTreeMap::new(),
                    None,
                    BTreeMap::new(),
                    None,
                    None,
                    Some(mode),
                )
                .await
        }));
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    let persisted = AppConfig::load(Some(&source)).await.unwrap();
    assert_eq!(server.permission_mode().await, persisted.permissions.mode);
    assert_eq!(
        server.config.read().await.permissions.mode,
        persisted.permissions.mode
    );
    server.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn atomic_save_preserves_the_profile_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.toml");
    let alias = dir.path().join("alias.toml");
    tokio::fs::write(&source, "[permissions]\nmode='normal'\n")
        .await
        .unwrap();
    std::os::unix::fs::symlink(&source, &alias).unwrap();
    let mut config = AppConfig::default();
    config.permissions.mode = PermissionMode::Plan;
    let target = config_builder_target_path(Some(&alias)).unwrap();
    persist_config_builder(&target, &config).await.unwrap();
    assert!(
        std::fs::symlink_metadata(&alias)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        AppConfig::load(Some(&source))
            .await
            .unwrap()
            .permissions
            .mode,
        PermissionMode::Plan
    );
}

#[tokio::test]
async fn first_profile_save_does_not_persist_a_session_mode_override() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("config.toml");
    let mut config = crate::test_model::config();
    config.permissions.mode = PermissionMode::Normal;
    let server = AgentAppServer::launch(config, home.path().into(), Some(&source))
        .await
        .unwrap();
    server.set_permission_mode(PermissionMode::Plan).await;
    assert!(!source.exists());
    assert_eq!(
        server.local_config_summary().await["permission_mode"],
        "Plan"
    );
    assert_eq!(
        server.config_builder_snapshot().await.permission_mode,
        "normal"
    );

    server
        .set_config_builder(BTreeMap::new(), None, BTreeMap::new(), None, None, None)
        .await
        .unwrap();

    assert_eq!(
        AppConfig::load(Some(&source))
            .await
            .unwrap()
            .permissions
            .mode,
        PermissionMode::Normal
    );
    assert_eq!(
        server.config.read().await.permissions.mode,
        PermissionMode::Normal
    );
    assert_eq!(server.permission_mode().await, PermissionMode::Plan);
    server.shutdown().await;
}

#[tokio::test]
async fn json_save_preserves_unmanaged_fields_and_updates_managed_profile() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profile.json");
    let mut initial = serde_json::to_value(AppConfig::default()).unwrap();
    initial["runtime"]["model_timeout_ms"] = serde_json::json!(98765);
    initial["agent_control"]["max_depth"] = serde_json::json!(3);
    tokio::fs::write(&path, serde_json::to_vec_pretty(&initial).unwrap())
        .await
        .unwrap();
    let mut config = AppConfig::load(Some(&path)).await.unwrap();
    config.permissions.mode = PermissionMode::Plan;
    config.tools.enabled = vec!["new-tool".into()];
    persist_config_builder(&path, &config).await.unwrap();
    let loaded = AppConfig::load(Some(&path)).await.unwrap();
    assert_eq!(loaded.permissions.mode, PermissionMode::Plan);
    assert_eq!(loaded.tools.enabled, config.tools.enabled);
    let saved: Value = serde_json::from_slice(&tokio::fs::read(&path).await.unwrap()).unwrap();
    assert_eq!(saved["runtime"]["model_timeout_ms"], 98765);
    assert_eq!(saved["agent_control"]["max_depth"], 3);
}

#[tokio::test]
async fn json_profile_save_does_not_apply_toml_value_restrictions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profile.json");
    let mut config = crate::test_model::config();
    config
        .module_config
        .entry("model".into())
        .or_default()
        .insert("unselected-model".into(), Value::Null);
    tokio::fs::write(&path, serde_json::to_vec_pretty(&config).unwrap())
        .await
        .unwrap();
    let server = AgentAppServer::launch(config, dir.path().into(), Some(&path))
        .await
        .unwrap();
    server
        .set_config_builder(
            BTreeMap::new(),
            None,
            BTreeMap::new(),
            None,
            None,
            Some(PermissionMode::Plan),
        )
        .await
        .unwrap();
    let loaded = AppConfig::load(Some(&path)).await.unwrap();
    assert_eq!(loaded.permissions.mode, PermissionMode::Plan);
    assert_eq!(
        loaded.module_config["model"]["unselected-model"],
        Value::Null
    );
    server.shutdown().await;
}
