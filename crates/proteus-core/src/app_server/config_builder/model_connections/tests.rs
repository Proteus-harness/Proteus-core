use super::*;
use crate::app_server::AgentAppServer;
use proteus_contracts::app_protocol::http::{ComponentPackageInstall, SetConfigBuilderRequest};
use serde_json::json;

fn package(shared: bool) -> ComponentPackageInstall {
    let mut exports = BTreeMap::from([("model".into(), vec!["spare".into()])]);
    if shared {
        exports.insert("tool".into(), vec!["direct_patch".into()]);
    }
    ComponentPackageInstall {
        id: "spare-connection".into(),
        command: vec![
            crate::test_model::reference_module()
                .to_string_lossy()
                .into_owned(),
        ],
        icon: None,
        env_allowlist: vec![],
        exports,
        initial_config: BTreeMap::from([(
            "model".into(),
            BTreeMap::from([("spare".into(), json!({"implementation":"fake"}))]),
        )]),
        replace: vec![],
    }
}

async fn write_source(path: &Path, initial: &AppConfig, include: bool) {
    let extension = path.extension().unwrap().to_str().unwrap();
    let value = if include {
        let base = path.with_file_name("base.toml");
        std::fs::write(base, toml::to_string(initial).unwrap()).unwrap();
        json!({"include":"base.toml"})
    } else {
        serde_json::to_value(initial).unwrap()
    };
    let bytes = if extension == "json" {
        serde_json::to_vec_pretty(&value).unwrap()
    } else if include {
        b"include = 'base.toml'\n".to_vec()
    } else {
        toml::to_string(initial).unwrap().into_bytes()
    };
    std::fs::write(path, bytes).unwrap();
}

#[tokio::test]
async fn model_connection_removal_persists_exact_export_and_preserves_shared_components() {
    for extension in ["toml", "json"] {
        for shared in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let profile = root.path().join(format!("profile.{extension}"));
            let initial = crate::test_model::config();
            write_source(&profile, &initial, true).await;
            let server = AgentAppServer::launch(initial, root.path().into(), Some(&profile))
                .await
                .unwrap();
            server
                .set_profile_config(SetConfigBuilderRequest {
                    component_packages: vec![package(shared)],
                    ..Default::default()
                })
                .await
                .unwrap();
            let snapshot = server
                .set_profile_config(SetConfigBuilderRequest {
                    remove_model_connections: vec!["spare".into()],
                    ..Default::default()
                })
                .await
                .unwrap();
            assert!(
                !snapshot
                    .plugins
                    .iter()
                    .flat_map(|p| &p.exports)
                    .any(|export| export.slot == "model" && export.id == "spare")
            );
            let loaded = AppConfig::load(Some(&profile)).await.unwrap();
            assert!(loaded.module_config["model"].get("spare").is_none());
            match loaded.components.get("spare-connection") {
                Some(component) => {
                    assert!(shared);
                    assert_eq!(
                        component
                            .exports()
                            .map(|(slot, id, _)| (slot, id))
                            .collect::<Vec<_>>(),
                        vec![("tool", "direct_patch")]
                    );
                }
                None => assert!(!shared),
            }
            assert!(loaded.components.contains_key("test-model"));
            server.shutdown().await;
            let restarted = AgentAppServer::launch(loaded, root.path().into(), Some(&profile))
                .await
                .unwrap();
            assert!(
                !restarted
                    .config_builder_snapshot()
                    .await
                    .plugins
                    .iter()
                    .flat_map(|plugin| &plugin.exports)
                    .any(|export| export.slot == "model" && export.id == "spare")
            );
            restarted.shutdown().await;
        }
    }
}

#[tokio::test]
async fn model_connection_removal_rejects_referenced_and_inherited_exports_without_writes() {
    for extension in ["toml", "json"] {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join(format!("profile.{extension}"));
        let mut initial = crate::test_model::config();
        initial.components.insert(
            "extra".into(),
            serde_json::from_value(json!({
                "command":crate::test_model::reference_module(),
                "exports":{"model":{"inherited":{}}}
            }))
            .unwrap(),
        );
        initial
            .module_config
            .entry("model".into())
            .or_default()
            .insert("inherited".into(), json!({"implementation":"fake"}));
        write_source(&profile, &initial, true).await;
        let before = std::fs::read(&profile).unwrap();
        let server = AgentAppServer::launch(initial, root.path().into(), Some(&profile))
            .await
            .unwrap();
        for (id, expected) in [
            ("fake", "used by model presets"),
            ("inherited", "inherited"),
        ] {
            let error = server
                .set_profile_config(SetConfigBuilderRequest {
                    remove_model_connections: vec![id.into()],
                    permission_mode: Some(PermissionMode::Auto),
                    ..Default::default()
                })
                .await
                .unwrap_err();
            assert!(format!("{error:#}").contains(expected), "{error:#}");
            assert_eq!(std::fs::read(&profile).unwrap(), before);
            assert_eq!(
                server.config_builder_snapshot().await.permission_mode,
                "normal"
            );
        }
        server.shutdown().await;
    }
}

#[tokio::test]
async fn model_connection_removal_detects_local_export_reintroduced_by_include() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("base.toml");
    let profile = root.path().join("profile.toml");
    let mut config = AppConfig::default();
    config.providers.clear();
    config.active_provider = None;
    let component = json!({"command":"/fixture", "exports":{"model":{"shared":{}}}});
    config.components.insert(
        "shared".into(),
        serde_json::from_value(component.clone()).unwrap(),
    );
    std::fs::write(&base, toml::to_string(&config).unwrap()).unwrap();
    std::fs::write(
        &profile,
        format!(
            "include='base.toml'\n{}",
            toml::to_string(&json!({"components":{"shared":component}})).unwrap()
        ),
    )
    .unwrap();
    let error = remove(&mut config, &profile, &["shared".into()]).unwrap_err();
    assert!(format!("{error:#}").contains("also inherited"));
}
