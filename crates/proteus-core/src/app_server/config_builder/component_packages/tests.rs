use super::*;
use crate::app_server::AgentAppServer;
use proteus_contracts::app_protocol::http::{ComponentReplacement, SetConfigBuilderRequest};
use serde_json::json;

fn package(command: &str) -> ComponentPackageInstall {
    ComponentPackageInstall {
        id: "installed-package".into(),
        command: vec![command.into(), "package-argument".into()],
        exports: BTreeMap::from([
            ("model".into(), vec!["fake".into()]),
            ("tool".into(), vec!["direct_patch".into()]),
        ]),
        replace: Vec::new(),
    }
}

fn replacement(id: &str, command: &str) -> ComponentReplacement {
    ComponentReplacement {
        id: id.into(),
        command: command.into(),
    }
}

#[test]
fn component_packages_preserve_alias_descriptors_and_only_replace_explicit_commands() {
    let descriptor = json!({
        "command": "/old/worker", "args": ["--custom"], "cwd": "workspace",
        "env_allowlist": ["LANG"], "env": {"FIXTURE": "yes"},
        "sandbox": {"mode": "restricted", "read_only": ["."], "read_write": [], "network": false},
        "handshake_timeout_ms": 4321, "description": "custom launch",
        "exports": {"tool": {"direct_patch": {"timeout_ms": 1234, "description": "custom export"}}}
    });
    let mut config = AppConfig::default();
    config
        .components
        .insert("custom".into(), serde_json::from_value(descriptor).unwrap());
    config.components.insert(
        "untouched".into(),
        serde_json::from_value(json!({
            "command": "/old/worker", "exports": {"model": {"fake": {}}},
        }))
        .unwrap(),
    );
    let mut before = serde_json::to_value(&config).unwrap();
    let mut install = package("/installed/worker");
    install.replace.push(replacement("custom", "/old/worker"));
    apply(&mut config, vec![install]).unwrap();
    before["components"]["custom"]["command"] = json!("/installed/worker");
    assert_eq!(serde_json::to_value(&config).unwrap(), before);
    assert!(!config.components.contains_key("installed-package"));
}

#[test]
fn component_packages_reject_stale_alias_mismatched_binary_exports_and_collisions() {
    let mut original = AppConfig::default();
    original.components.insert(
        "custom".into(),
        serde_json::from_value(json!({
            "command": "/old/worker", "exports": {"model": {"fake": {}}},
        }))
        .unwrap(),
    );
    for mut install in [package("/new/worker"), package("/new/different")] {
        install.replace.push(replacement("custom", "/stale/worker"));
        assert!(apply(&mut original.clone(), vec![install.clone()]).is_err());
        install.replace[0].command = "/old/worker".into();
        install.exports.remove("model");
        assert!(apply(&mut original.clone(), vec![install]).is_err());
    }
    let mut collision = package("/new/worker");
    collision.id = "custom".into();
    assert!(apply(&mut original.clone(), vec![collision]).is_err());
    for command in [
        vec![],
        vec!["relative-worker".into()],
        vec!["/tmp/worker\0".into()],
    ] {
        let mut install = package("/new/worker");
        install.command = command;
        assert!(apply(&mut original.clone(), vec![install]).is_err());
    }
    let mut duplicate = package("/new/worker");
    duplicate
        .exports
        .insert("model".into(), vec!["fake".into(), "fake".into()]);
    assert!(apply(&mut original, vec![duplicate]).is_err());
}

#[test]
fn component_packages_request_roundtrips_and_empty_registration_is_omitted() {
    let empty = SetConfigBuilderRequest::default();
    assert!(
        serde_json::to_value(empty)
            .unwrap()
            .get("component_packages")
            .is_none()
    );
    let install = package("/new/worker");
    let request = SetConfigBuilderRequest {
        component_packages: vec![install.clone()],
        ..Default::default()
    };
    let loaded: SetConfigBuilderRequest =
        serde_json::from_value(serde_json::to_value(request).unwrap()).unwrap();
    assert_eq!(loaded.component_packages, [install]);
}

#[tokio::test]
async fn component_packages_ordinary_save_does_not_freeze_inherited_descriptors() {
    let home = tempfile::tempdir().unwrap();
    let fragment = home.path().join("source.toml");
    let source = "[components.inherited]\ncommand='/old/worker'\n[components.inherited.exports.tool.fixture]\n";
    std::fs::write(&fragment, source).unwrap();
    let config = AppConfig::load(Some(home.path())).await.unwrap();
    let overlay = config_builder_target_path(Some(home.path())).unwrap();
    persist_config_builder(&overlay, &config).await.unwrap();
    let saved: toml::Value = toml::from_str(&std::fs::read_to_string(&overlay).unwrap()).unwrap();
    assert!(saved.get("components").is_none());
    std::fs::write(&fragment, source.replace("/old/worker", "/edited/worker")).unwrap();
    let reloaded = AppConfig::load(Some(home.path())).await.unwrap();
    assert_eq!(reloaded.components["inherited"].command(), "/edited/worker");
}

#[tokio::test]
async fn component_packages_transaction_updates_aliases_persists_new_exports_and_cold_reloads() {
    let home = tempfile::tempdir().unwrap();
    let profiles = home.path().join("profiles");
    std::fs::create_dir(&profiles).unwrap();
    let initial = crate::test_model::config();
    std::fs::write(
        profiles.join("z-source.toml"),
        toml::to_string_pretty(&initial).unwrap(),
    )
    .unwrap();
    let old = crate::test_model::reference_module();
    let new_dir = home.path().join("installed");
    std::fs::create_dir(&new_dir).unwrap();
    let executable = new_dir.join(old.file_name().unwrap());
    std::fs::copy(&old, &executable).unwrap();
    let server = AgentAppServer::launch(initial.clone(), home.path().into(), Some(&profiles))
        .await
        .unwrap();
    let mut update = package(executable.to_str().unwrap());
    update
        .replace
        .push(replacement("test-model", old.to_str().unwrap()));
    let mut extra = package(old.to_str().unwrap());
    extra.id = "new-tool-package".into();
    extra.command.truncate(1);
    extra.exports.remove("model");
    let snapshot = server
        .set_profile_config(SetConfigBuilderRequest {
            component_packages: vec![update, extra],
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(snapshot.plugins.iter().any(|plugin| {
        plugin.id == "new-tool-package"
            && plugin
                .exports
                .iter()
                .any(|export| export.slot == "tool" && export.id == "direct_patch")
    }));
    let loaded = AppConfig::load(Some(&profiles)).await.unwrap();
    assert_eq!(
        loaded.components["test-model"].command(),
        executable.to_str().unwrap()
    );
    assert_eq!(
        serde_json::to_value(&loaded.modules).unwrap(),
        serde_json::to_value(&initial.modules).unwrap()
    );
    assert_eq!(loaded.tools.enabled, initial.tools.enabled);
    assert_eq!(loaded.module_config, initial.module_config);
    assert_eq!(loaded.components.len(), 2);
    server.shutdown().await;
    let restarted = AgentAppServer::launch(loaded.clone(), home.path().into(), Some(&profiles))
        .await
        .unwrap();
    let snapshot = restarted.config_builder_snapshot().await;
    assert!(
        snapshot
            .plugins
            .iter()
            .any(|plugin| plugin.id == "test-model"
                && plugin.command == executable.to_str().unwrap())
    );
    assert!(
        snapshot
            .plugins
            .iter()
            .any(|plugin| plugin.id == "new-tool-package")
    );
    restarted.shutdown().await;
}

#[tokio::test]
async fn component_packages_failed_prepare_keeps_runtime_and_profile_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let profile = home.path().join("config.toml");
    let initial = crate::test_model::config();
    std::fs::write(&profile, toml::to_string_pretty(&initial).unwrap()).unwrap();
    let before = std::fs::read(&profile).unwrap();
    let server = AgentAppServer::launch(initial.clone(), home.path().into(), Some(&profile))
        .await
        .unwrap();
    let mut install = package(crate::test_model::reference_module().to_str().unwrap());
    install.exports = BTreeMap::from([("unsupported-slot".into(), vec!["bad".into()])]);
    let error = server
        .set_profile_config(SetConfigBuilderRequest {
            component_packages: vec![install],
            permission_mode: Some(PermissionMode::Auto),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("unsupported"));
    assert_eq!(std::fs::read(&profile).unwrap(), before);
    assert_eq!(
        serde_json::to_value(&*server.config.read().await).unwrap(),
        serde_json::to_value(&initial).unwrap()
    );
    assert_eq!(server.config_builder_snapshot().await.plugins.len(), 1);
    assert!(!home.path().join("config-history").exists());
    server.shutdown().await;
}
