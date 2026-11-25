use super::*;
use crate::app_server::AppServerEvent;
use proteus_contracts::app_protocol::http::SetConfigBuilderRequest;

fn fixture_config() -> AppConfig {
    let mut config = AppConfig::default();
    config.active_provider = None;
    config.providers.clear();
    config.agent_control.surface = crate::core::AgentControlSurface::None;
    config.tools.enabled = vec!["probe_command".into()];
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/process_commands.py");
    config.components.insert(
        "probe".into(),
        serde_json::from_value(serde_json::json!({
            "command": "python3", "args": [fixture],
            "exports": {"tool": {"probe.commands": {}}}
        }))
        .unwrap(),
    );
    config
}

fn write_config(path: &Path, config: &AppConfig) {
    let bytes = if path
        .extension()
        .is_some_and(|extension| extension == "json")
    {
        serde_json::to_vec_pretty(config).unwrap()
    } else {
        toml::to_string_pretty(config).unwrap().into_bytes()
    };
    std::fs::write(path, bytes).unwrap();
}

async fn launch(
    root: &Path,
    format: &str,
    config: AppConfig,
) -> (PathBuf, PathBuf, AppServerHandle) {
    let source = root.join(format!("config.{format}"));
    let target = if format == "directory" {
        std::fs::create_dir(&source).unwrap();
        write_config(&source.join("base.toml"), &config);
        config_builder_target_path(Some(&source)).unwrap()
    } else {
        if !format.starts_with("missing.") {
            write_config(&source, &config);
        }
        source.clone()
    };
    let config = if source.exists() {
        AppConfig::load(Some(&source)).await.unwrap()
    } else {
        config
    };
    let cwd = root.join("workspace");
    std::fs::create_dir(&cwd).unwrap();
    let server = AgentAppServer::launch(config, cwd, Some(&source))
        .await
        .unwrap();
    (source, target, server)
}

#[tokio::test]
async fn invalid_save_preserves_disk_and_live_epoch_then_valid_save_succeeds() {
    for format in ["toml", "json", "directory", "missing.toml", "missing.json"] {
        let root = tempfile::tempdir().unwrap();
        let (source, target, server) = launch(root.path(), format, fixture_config()).await;
        let disk = std::fs::read(&target).ok();
        let live = serde_json::to_value(server.config.read().await.as_ref()).unwrap();
        let epoch = server.runtime.module_epoch().await;
        for (request, message) in [
            (
                serde_json::json!({"module_config":{"unknown_slot":{"unused":{"value":1}}}}),
                "unknown module_config slot",
            ),
            (
                serde_json::json!({"hooks":["same", "same"]}),
                "duplicate id",
            ),
            (serde_json::json!({"hooks":[" "]}), "blank id"),
            (serde_json::json!({"model":"missing"}), "not defined"),
            (serde_json::json!({"model":" "}), "must not be empty"),
        ] {
            let request: SetConfigBuilderRequest = serde_json::from_value(request).unwrap();
            let error = server.set_profile_config(request).await.unwrap_err();
            assert!(
                format!("{error:#}").contains(message),
                "{format}: {error:#}"
            );
            assert_eq!(std::fs::read(&target).ok(), disk, "{format}: disk changed");
            assert_eq!(
                serde_json::to_value(server.config.read().await.as_ref()).unwrap(),
                live
            );
            assert_eq!(server.runtime.module_epoch().await, epoch);
        }
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
        assert_eq!(
            AppConfig::load(Some(&source))
                .await
                .unwrap()
                .permissions
                .mode,
            PermissionMode::Plan
        );
        assert_eq!(server.permission_mode().await, PermissionMode::Plan);
        server.shutdown().await;
    }
}

#[tokio::test]
async fn empty_profile_save_remains_model_free_and_cold_loadable() {
    for format in ["toml", "json", "directory"] {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join(format!("empty.{format}"));
        match format {
            "directory" => std::fs::create_dir(&source).unwrap(),
            "json" => std::fs::write(&source, "{}").unwrap(),
            _ => std::fs::write(&source, "").unwrap(),
        }
        let config = AppConfig::load(Some(&source)).await.unwrap();
        assert!(config.active_provider.is_none());
        assert!(config.components.is_empty());
        let initial = serde_json::to_value(&config).unwrap();
        let server = AgentAppServer::launch(config, root.path().into(), Some(&source))
            .await
            .unwrap();
        server
            .set_profile_config(serde_json::from_value(serde_json::json!({})).unwrap())
            .await
            .unwrap();
        let cold = AppConfig::load(Some(&source)).await.unwrap();
        assert_eq!(serde_json::to_value(cold).unwrap(), initial, "{format}");
        assert_eq!(
            serde_json::to_value(server.config.read().await.as_ref()).unwrap(),
            initial
        );
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
        let cold = AppConfig::load(Some(&source)).await.unwrap();
        assert!(cold.active_provider.is_none());
        assert!(cold.components.is_empty());
        assert_eq!(cold.permissions.mode, PermissionMode::Plan);
        server.shutdown().await;
    }
}

#[tokio::test]
async fn supported_slot_save_preserves_resolved_file_instructions_and_cold_load() {
    for (format, relative) in ["toml", "json", "directory"]
        .into_iter()
        .flat_map(|format| [(format, false), (format, true)])
    {
        let root = tempfile::tempdir().unwrap();
        let mut config = serde_json::to_value(fixture_config()).unwrap();
        let prompt = root.path().join("instructions.md");
        std::fs::write(&prompt, "File instructions.").unwrap();
        let instruction_path = if relative {
            PathBuf::from(if format == "directory" {
                "../instructions.md"
            } else {
                "instructions.md"
            })
        } else {
            prompt
        };
        config["instructions"] = serde_json::json!([
            {"kind":"System", "file":instruction_path, "priority":100},
            {"kind":"Developer", "text":"Inline instructions.", "priority":90}
        ]);
        let (source, _, server) =
            launch(root.path(), format, serde_json::from_value(config).unwrap()).await;
        let instructions = server.config.read().await.instructions.clone();
        assert_eq!(instructions[0].file.as_ref(), Some(&instruction_path));
        assert_eq!(instructions[0].text.as_deref(), Some("File instructions."));
        server
            .set_config_builder(
                BTreeMap::new(),
                None,
                BTreeMap::from([(
                    "tool".into(),
                    BTreeMap::from([("unused".into(), serde_json::json!({"value":1}))]),
                )]),
                None,
                None,
                None,
            )
            .await
            .unwrap();
        let cold = AppConfig::load(Some(&source)).await.unwrap();
        assert_eq!(cold.module_config["tool"]["unused"]["value"], 1);
        assert_eq!(
            serde_json::to_value(cold.instructions).unwrap(),
            serde_json::to_value(&instructions).unwrap()
        );
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
        assert_eq!(
            AppConfig::load(Some(&source))
                .await
                .unwrap()
                .permissions
                .mode,
            PermissionMode::Plan
        );
        server.shutdown().await;
    }
}

#[tokio::test]
async fn watcher_rejects_same_invalid_slot_and_recovers_without_replacing_valid_runtime() {
    for format in ["toml", "json"] {
        let root = tempfile::tempdir().unwrap();
        let config = fixture_config();
        let (source, _, server) = launch(root.path(), format, config.clone()).await;
        let epoch = server.runtime.module_epoch().await;
        let mut events = server.subscribe();
        let mut invalid = config.clone();
        invalid
            .module_config
            .insert("unknown_slot".into(), BTreeMap::new());
        write_config(&source, &invalid);
        let expected_error = AppConfig::load(Some(&source))
            .await
            .unwrap_err()
            .to_string();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let AppServerEvent::ProfileReloadStatus { error: Some(error) } =
                    events.recv().await.unwrap()
                {
                    assert_eq!(error, expected_error);
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(server.runtime.module_epoch().await, epoch);
        assert!(
            !server
                .config
                .read()
                .await
                .module_config
                .contains_key("unknown_slot")
        );
        let mut recovered = config;
        recovered.permissions.mode = PermissionMode::Plan;
        write_config(&source, &recovered);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
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
        assert_eq!(server.permission_mode().await, PermissionMode::Plan);
        assert!(server.addons_snapshot().await.reload_error.is_none());
        server.shutdown().await;
    }
}
