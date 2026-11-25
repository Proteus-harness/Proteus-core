use super::*;
use crate::{
    app_server::AgentAppServer,
    app_server::AppServerEvent,
    contracts::CancellationToken,
    domain::{PermissionMode, ToolCall, new_call_id},
};

#[test]
fn package_fingerprint_tracks_semantic_changes_and_keeps_only_digests() {
    let root = tempfile::tempdir().unwrap();
    let mut config = crate::test_model::config();
    config
        .addons
        .plugins
        .push(crate::domain::AgentPluginConfig {
            path: root.path().to_owned(),
            enabled: false,
            data_key: None,
        });
    let manifest = root.path().join("plugin.json");
    std::fs::write(&manifest, r#"{"name":"test","version":"1"}"#).unwrap();
    let original = Fingerprint::new(&config, root.path()).unwrap();
    std::fs::write(&manifest, r#"{ "name": "test", "version": "1" }"#).unwrap();
    assert!(original == Fingerprint::new(&config, root.path()).unwrap());
    std::fs::write(root.path().join("mcp.json"), r#"{"servers":{}}"#).unwrap();
    let changed = Fingerprint::new(&config, root.path()).unwrap();
    assert_ne!(original.packages, changed.packages);
    assert_eq!(original.config, changed.config);
    assert_eq!(std::mem::size_of::<Fingerprint>(), 64);
}

async fn next_reload(events: &mut tokio::sync::broadcast::Receiver<AppServerEvent>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                events.recv().await.unwrap(),
                AppServerEvent::ModulesReloaded { .. }
            ) {
                break;
            }
        }
    })
    .await
    .unwrap();
}

async fn next_error(events: &mut tokio::sync::broadcast::Receiver<AppServerEvent>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                events.recv().await.unwrap(),
                AppServerEvent::ProfileReloadStatus { error: Some(_) }
            ) {
                break;
            }
        }
    })
    .await
    .unwrap();
}

async fn skill(handle: &AppServerHandle) -> crate::domain::ToolResult {
    handle
        .runtime
        .execute_tool(
            ToolCall::new(new_call_id(), "skill", serde_json::json!({"name":"review"})),
            CancellationToken::new(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn file_and_ui_updates_reach_other_open_sessions_and_bad_source_preserves_the_epoch() {
    let workspace = tempfile::tempdir().unwrap();
    let configs = tempfile::tempdir().unwrap();
    let path = configs.path().join("config.toml");
    let fragment = configs.path().join("selection.toml");
    std::fs::create_dir(workspace.path().join(".git")).unwrap();
    let skills = workspace.path().join(".proteus/skills/review");
    std::fs::create_dir_all(&skills).unwrap();
    std::fs::write(
        skills.join("SKILL.md"),
        "---\nname: review\ndescription: Test\n---\nInstructions.\n",
    )
    .unwrap();
    let mut config = crate::test_model::config();
    config.permissions.mode = PermissionMode::Normal;
    config.components.insert("capabilities".into(),serde_json::from_value(serde_json::json!({
        "command":crate::test_model::reference_module(),"exports":{"tool":{"reference.tools":{}},"context_provider":{"skills":{}},"policy":{"allow_all":{}}}
    })).unwrap());
    config.modules.policy = Some("allow_all".into());
    config.tools.enabled = vec!["skill".into()];
    let mut document = toml::Value::try_from(&config).unwrap();
    document.as_table_mut().unwrap().remove("modules");
    document.as_table_mut().unwrap().remove("addons");
    let source = format!(
        "include = 'selection.toml'\n{}",
        toml::to_string_pretty(&document).unwrap()
    );
    std::fs::write(&path, &source).unwrap();
    let selection = "[modules]\npolicy='allow_all'\n";
    std::fs::write(&fragment, selection).unwrap();
    let first = AgentAppServer::launch(
        AppConfig::load(Some(&path)).await.unwrap(),
        workspace.path().to_path_buf(),
        Some(&path),
    )
    .await
    .unwrap();
    let second = AgentAppServer::launch(
        AppConfig::load(Some(&path)).await.unwrap(),
        workspace.path().to_path_buf(),
        Some(&path),
    )
    .await
    .unwrap();
    let mut first_events = first.subscribe();
    let mut second_events = second.subscribe();
    first
        .set_permission_mode(PermissionMode::Auto)
        .await
        .unwrap();
    second
        .set_permission_mode(PermissionMode::Plan)
        .await
        .unwrap();
    assert!(skill(&first).await.ok);
    assert!(skill(&second).await.ok);
    // Includes are read through the same parser, not a special watcher schema.
    std::fs::write(
        &fragment,
        format!("{selection}[addons]\ndisabled_skills=['review']\n"),
    )
    .unwrap();
    tokio::join!(
        next_reload(&mut first_events),
        next_reload(&mut second_events)
    );
    assert!(!skill(&first).await.ok);
    assert!(!skill(&second).await.ok);
    assert_eq!(first.permission_mode().await, PermissionMode::Auto);
    assert_eq!(second.permission_mode().await, PermissionMode::Plan);
    let epoch = first.runtime.module_epoch().await;
    std::fs::write(&fragment, "[modules]\npolicy='not-registered'\n").unwrap();
    tokio::join!(
        next_error(&mut first_events),
        next_error(&mut second_events)
    );
    assert_eq!(first.runtime.module_epoch().await, epoch);
    assert_eq!(first.permission_mode().await, PermissionMode::Auto);
    assert_eq!(second.permission_mode().await, PermissionMode::Plan);
    assert!(
        !skill(&first).await.ok,
        "bad profile must not undo the last valid availability"
    );
    assert!(first.addons_snapshot().await.reload_error.is_some());
    std::fs::write(&fragment, selection).unwrap();
    tokio::join!(
        next_reload(&mut first_events),
        next_reload(&mut second_events)
    );
    assert!(skill(&first).await.ok);
    assert!(first.addons_snapshot().await.reload_error.is_none());
    assert_eq!(first.permission_mode().await, PermissionMode::Auto);
    assert_eq!(second.permission_mode().await, PermissionMode::Plan);
    let mut update = first.addons_snapshot().await.settings;
    update.addons.disabled_skills = vec!["review".into()];
    first.set_addons(update).await.unwrap();
    next_reload(&mut second_events).await;
    assert!(
        !skill(&second).await.ok,
        "UI save must propagate through the profile to another session"
    );
    assert_eq!(first.permission_mode().await, PermissionMode::Auto);
    assert_eq!(second.permission_mode().await, PermissionMode::Plan);
    assert_eq!(
        AppConfig::load(Some(&path)).await.unwrap().permissions.mode,
        PermissionMode::Normal,
        "unrelated saves must not persist either session's override"
    );

    // An actual profile-default change still replaces both session overrides.
    second
        .set_permission_mode(PermissionMode::Normal)
        .await
        .unwrap();
    first
        .set_config_builder(
            Default::default(),
            None,
            Default::default(),
            None,
            None,
            Some(PermissionMode::Plan),
        )
        .await
        .unwrap();
    next_reload(&mut second_events).await;
    assert_eq!(first.permission_mode().await, PermissionMode::Plan);
    assert_eq!(second.permission_mode().await, PermissionMode::Plan);
    first.shutdown().await;
    second.shutdown().await;
}

#[tokio::test]
async fn instruction_file_edits_reload_without_touching_the_profile() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.toml");
    let rules = root.path().join("rules.md");
    std::fs::write(&rules, "first rules").unwrap();
    let mut config = crate::test_model::config();
    config.instructions = vec![
        serde_json::from_value(serde_json::json!({
            "kind":"System", "priority":100, "file":"rules.md"
        }))
        .unwrap(),
    ];
    std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
    let server = AgentAppServer::launch(
        AppConfig::load(Some(&path)).await.unwrap(),
        root.path().to_path_buf(),
        Some(&path),
    )
    .await
    .unwrap();
    let mut events = server.subscribe();
    // Let both debounce reads settle before editing only the external input.
    tokio::time::sleep(Duration::from_millis(750)).await;
    let epoch = server.runtime.module_epoch().await;
    std::fs::write(&rules, "other rules").unwrap();
    next_reload(&mut events).await;
    assert!(server.runtime.module_epoch().await > epoch);
    assert_eq!(
        server.config.read().await.instructions[0].text.as_deref(),
        Some("other rules")
    );
    server.shutdown().await;
}

#[tokio::test]
async fn watcher_does_not_keep_a_dropped_handle_alive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let handle = AgentAppServer::launch(
        crate::test_model::config(),
        dir.path().to_path_buf(),
        Some(&path),
    )
    .await
    .unwrap();
    let owner = std::sync::Arc::downgrade(&handle.inner);
    drop(handle);
    tokio::time::timeout(Duration::from_secs(2), async {
        while owner.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn inline_role_watches_physical_source_and_ui_save_keeps_parent_and_siblings() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent.json");
    let child = serde_json::to_value(crate::test_model::config()).unwrap();
    let sibling = child.clone();
    let mut document = child.clone();
    document["permissions"]["mode"] = serde_json::json!("plan");
    document["agent_control"]["roles"] = serde_json::json!([
        { "name": "explore", "description": "Reader", "config": child },
        { "name": "coder", "description": "Writer", "config": sibling }
    ]);
    std::fs::write(&path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let selected = crate::core::config_reference(&path, "explore").unwrap();
    let server = AgentAppServer::launch(
        AppConfig::load(Some(&selected)).await.unwrap(),
        directory.path().to_path_buf(),
        Some(&selected),
    )
    .await
    .unwrap();
    let mut events = server.subscribe();
    document["agent_control"]["roles"][0]["config"]["permissions"]["mode"] =
        serde_json::json!("auto");
    std::fs::write(&path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    next_reload(&mut events).await;
    assert_eq!(
        server.config.read().await.permissions.mode,
        PermissionMode::Auto
    );
    assert_eq!(server.permission_mode().await, PermissionMode::Auto);

    server
        .set_config_builder(
            Default::default(),
            None,
            Default::default(),
            None,
            None,
            Some(PermissionMode::Normal),
        )
        .await
        .unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["permissions"]["mode"], "plan");
    assert_eq!(
        saved["agent_control"]["roles"][1],
        document["agent_control"]["roles"][1]
    );
    assert_eq!(
        saved["agent_control"]["roles"][0]["config"]["permissions"]["mode"],
        "normal"
    );
    assert_eq!(
        AppConfig::load(Some(&selected))
            .await
            .unwrap()
            .permissions
            .mode,
        PermissionMode::Normal
    );
    assert!(!selected.exists());
    server.shutdown().await;
}
