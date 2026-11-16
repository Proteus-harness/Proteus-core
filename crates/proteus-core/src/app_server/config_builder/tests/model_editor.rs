use super::*;
use proteus_contracts::app_protocol::http::SetConfigBuilderRequest;
use serde_json::json;

async fn launch_named(root: &Path, extension: &str) -> (PathBuf, AppServerHandle) {
    let path = root.join(format!("models.{extension}"));
    let mut source = serde_json::to_value(crate::test_model::config()).unwrap();
    source.as_object_mut().unwrap().remove("active_provider");
    source.as_object_mut().unwrap().remove("providers");
    source["model"] = json!("main");
    source["components"]["test-model"]["icon"] = json!("assets/model.svg");
    source["models"] = json!({
        "main":{"provider":"fake","model":"large","effort":"high"},
        "cheap":{"provider":"fake","model":"small","effort":"low"},
        "same-values":{"provider":"fake","model":"small","effort":"low"}
    });
    source["agent_control"]["roles"] = json!([
        {"name":"inherit","description":"current parent","config":{}},
        {"name":"fixed","description":"source alias","model":"cheap","config":{}}
    ]);
    fn omit_nulls(value: &mut Value) {
        match value {
            Value::Object(fields) => {
                fields.retain(|_, value| !value.is_null());
                for value in fields.values_mut() {
                    omit_nulls(value);
                }
            }
            Value::Array(values) => {
                for value in values {
                    omit_nulls(value);
                }
            }
            _ => {}
        }
    }
    omit_nulls(&mut source);
    let bytes = if extension == "json" {
        serde_json::to_vec_pretty(&source).unwrap()
    } else {
        format!(
            "# keep owner comment\n{}",
            toml::to_string_pretty(&source).unwrap().replace(
                "model = \"large\"",
                "model = \"large\" # keep unrelated model comment"
            )
        )
        .into_bytes()
    };
    std::fs::write(&path, bytes).unwrap();
    let config = AppConfig::load(Some(&path)).await.unwrap();
    let server = AgentAppServer::launch(config, root.into(), Some(&path))
        .await
        .unwrap();
    (path, server)
}

#[tokio::test]
async fn model_definitions_and_exact_role_choices_round_trip_and_restore_from_history() {
    for extension in ["toml", "json"] {
        let root = tempfile::tempdir().unwrap();
        let (path, server) = launch_named(root.path(), extension).await;
        let original = server.config_builder_snapshot().await;
        assert!(original.models_writable);
        assert_eq!(
            original
                .plugins
                .iter()
                .find(|plugin| plugin.id == "test-model")
                .unwrap()
                .icon
                .as_deref(),
            Some("assets/model.svg")
        );
        assert_eq!(
            original
                .model_roles
                .iter()
                .find(|role| role.name == "fixed")
                .unwrap()
                .model
                .as_deref(),
            Some("cheap")
        );
        assert_eq!(
            original
                .model_roles
                .iter()
                .find(|role| role.name == "inherit")
                .unwrap()
                .model,
            None
        );
        let mut models = original.models.clone();
        let cheap = models.get_mut("cheap").unwrap();
        cheap.model = "small-v2".into();
        cheap.stream = false;
        cheap.reasoning = crate::domain::ReasoningConfig::new(Some("high".into()), true)
            .with_budget_tokens(Some(73));
        cheap.reasoning_efforts = vec!["low".into(), "high".into()];
        let fast = cheap.clone();
        models.insert("fast-check".into(), fast);
        let changed = server
            .set_profile_config(SetConfigBuilderRequest {
                models: Some(models),
                role_models: Some(BTreeMap::from([
                    ("inherit".into(), Some("fast-check".into())),
                    ("fixed".into(), None),
                ])),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(changed.model.as_deref(), Some("main"));
        let loaded = AppConfig::load(Some(&path)).await.unwrap();
        assert_eq!(
            loaded.agent_control.roles[0].model.as_ref().unwrap().model,
            "small-v2"
        );
        assert_eq!(
            loaded.agent_control.roles[0]
                .model
                .as_ref()
                .unwrap()
                .reasoning
                .budget_tokens,
            Some(73)
        );
        assert!(loaded.agent_control.roles[1].model.is_none());
        if extension == "toml" {
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(
                text.contains("# keep owner comment")
                    && text.contains("# keep unrelated model comment")
            );
        }

        let mut unused_deleted = changed.models.clone();
        unused_deleted.remove("cheap");
        unused_deleted.remove("same-values");
        server
            .set_profile_config(SetConfigBuilderRequest {
                models: Some(unused_deleted.clone()),
                ..Default::default()
            })
            .await
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let epoch = server.runtime.module_epoch().await;
        let history = server.config_history().await.unwrap();
        unused_deleted.remove("fast-check");
        assert!(
            server
                .set_profile_config(SetConfigBuilderRequest {
                    models: Some(unused_deleted),
                    ..Default::default()
                })
                .await
                .unwrap_err()
                .to_string()
                .contains("unknown model alias")
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(server.runtime.module_epoch().await, epoch);
        assert_eq!(server.config_history().await.unwrap(), history);

        let state = history.revisions.last().unwrap().state.clone();
        let restored = server
            .set_profile_config(SetConfigBuilderRequest {
                model: Some(state.model),
                models: Some(state.models),
                role_models: Some(state.role_models),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(restored.models, original.models);
        assert_eq!(restored.model_roles, original.model_roles);
        let loaded = AppConfig::load(Some(&path)).await.unwrap();
        assert!(loaded.agent_control.roles[0].model.is_none());
        assert_eq!(
            loaded.agent_control.roles[1].model.as_ref().unwrap().model,
            "small"
        );
        server.shutdown().await;
    }
}

#[tokio::test]
async fn inherited_catalogue_and_external_roles_are_read_only_without_source_or_history_writes() {
    let root = tempfile::tempdir().unwrap();
    let (owner, owner_server) = launch_named(root.path(), "json").await;
    owner_server.shutdown().await;
    let mut value: Value = serde_json::from_slice(&std::fs::read(&owner).unwrap()).unwrap();
    value["agent_control"]["roles"][1]["config"] = json!("external.config.toml");
    std::fs::write(&owner, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let wrapper = root.path().join("wrapper.json");
    std::fs::write(
        &wrapper,
        serde_json::to_vec(&json!({"include":"models.json"})).unwrap(),
    )
    .unwrap();
    let config = AppConfig::load(Some(&wrapper)).await.unwrap();
    let server = AgentAppServer::launch(config, root.path().into(), Some(&wrapper))
        .await
        .unwrap();
    let snapshot = server.config_builder_snapshot().await;
    assert!(!snapshot.models_writable);
    assert!(snapshot.model_roles.iter().all(|role| !role.editable));
    let before = std::fs::read(&wrapper).unwrap();
    let owner_before = std::fs::read(&owner).unwrap();
    let mut models = snapshot.models;
    models.get_mut("cheap").unwrap().model = "other".into();
    assert!(
        server
            .set_profile_config(SetConfigBuilderRequest {
                models: Some(models),
                ..Default::default()
            })
            .await
            .unwrap_err()
            .to_string()
            .contains("owning source")
    );
    assert!(
        server
            .set_profile_config(SetConfigBuilderRequest {
                role_models: Some(BTreeMap::from([("fixed".into(), Some("main".into()))])),
                ..Default::default()
            })
            .await
            .unwrap_err()
            .to_string()
            .contains("read-only")
    );
    assert_eq!(std::fs::read(&wrapper).unwrap(), before);
    assert_eq!(std::fs::read(&owner).unwrap(), owner_before);
    assert!(server.config_history().await.unwrap().revisions.is_empty());
    server.shutdown().await;
}

#[tokio::test]
async fn inline_child_model_save_moves_outer_role_choice_before_normalization() {
    for extension in ["toml", "json"] {
        let root = tempfile::tempdir().unwrap();
        let (path, parent) = launch_named(root.path(), extension).await;
        parent.shutdown().await;
        let reference = crate::core::config_reference(&path, "fixed").unwrap();
        let child = AppConfig::load(Some(&reference)).await.unwrap();
        let server = AgentAppServer::launch(child, root.path().into(), Some(&reference))
            .await
            .unwrap();
        let changed = server
            .set_profile_config(SetConfigBuilderRequest {
                model: Some(Some("main".into())),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(changed.model.as_deref(), Some("main"));
        let reloaded = AppConfig::load(Some(&reference)).await.unwrap();
        assert_eq!(reloaded.active_model_config().unwrap().model, "large");
        let text = std::fs::read_to_string(&path).unwrap();
        let source: Value = if extension == "json" {
            serde_json::from_str(&text).unwrap()
        } else {
            serde_json::to_value(toml::from_str::<toml::Value>(&text).unwrap()).unwrap()
        };
        assert!(source["agent_control"]["roles"][1].get("model").is_none());
        assert_eq!(
            source["agent_control"]["roles"][1]["config"]["model"],
            "main"
        );
        server.shutdown().await;
    }
}

#[tokio::test]
async fn child_history_restores_inheritance_without_pinning_the_bootstrap_alias() {
    for extension in ["toml", "json"] {
        let root = tempfile::tempdir().unwrap();
        let (path, parent) = launch_named(root.path(), extension).await;
        parent.shutdown().await;
        let reference = crate::core::config_reference(&path, "inherit").unwrap();
        let child = AppConfig::load(Some(&reference)).await.unwrap();
        let server = AgentAppServer::launch(child, root.path().into(), Some(&reference))
            .await
            .unwrap();
        let original = server.config_builder_snapshot().await;
        assert_eq!(original.model, None);
        let changed = server
            .set_profile_config(SetConfigBuilderRequest {
                model: Some(Some("cheap".into())),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(changed.model.as_deref(), Some("cheap"));
        let old = server
            .config_history()
            .await
            .unwrap()
            .revisions
            .last()
            .unwrap()
            .state
            .clone();
        assert_eq!(old.model, None);
        let restored = server
            .set_profile_config(SetConfigBuilderRequest {
                model: Some(old.model),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(restored.model, None);
        let loaded = AppConfig::load(Some(&path)).await.unwrap();
        assert!(loaded.agent_control.roles[0].model.is_none());
        let child = AppConfig::load(Some(&reference)).await.unwrap();
        assert_eq!(child.active_provider.as_deref(), Some("main"));
        server.shutdown().await;
    }
}

#[tokio::test]
async fn present_null_clears_owned_legacy_root_model_selection() {
    for extension in ["toml", "json"] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(format!("legacy.{extension}"));
        let initial = crate::test_model::config();
        let bytes = if extension == "json" {
            serde_json::to_vec_pretty(&initial).unwrap()
        } else {
            toml::to_string_pretty(&initial).unwrap().into_bytes()
        };
        std::fs::write(&path, bytes).unwrap();
        let config = AppConfig::load(Some(&path)).await.unwrap();
        let server = AgentAppServer::launch(config, root.path().into(), Some(&path))
            .await
            .unwrap();
        let cleared = server
            .set_profile_config(SetConfigBuilderRequest {
                model: Some(None),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(cleared.model, None);
        assert_eq!(
            AppConfig::load(Some(&path)).await.unwrap().active_provider,
            None
        );
        server.shutdown().await;
    }
}

#[tokio::test]
async fn editing_nested_reasoning_preserves_the_distinct_flat_effort_field() {
    for extension in ["toml", "json"] {
        let root = tempfile::tempdir().unwrap();
        let (path, server) = launch_named(root.path(), extension).await;
        server.shutdown().await;
        let text = std::fs::read_to_string(&path).unwrap();
        let mut source: Value = if extension == "json" {
            serde_json::from_str(&text).unwrap()
        } else {
            serde_json::to_value(toml::from_str::<toml::Value>(&text).unwrap()).unwrap()
        };
        source["models"]["main"]["reasoning"] = json!({"summary":true});
        let bytes = if extension == "json" {
            serde_json::to_vec_pretty(&source).unwrap()
        } else {
            toml::to_string_pretty(&source).unwrap().into_bytes()
        };
        std::fs::write(&path, bytes).unwrap();
        let config = AppConfig::load(Some(&path)).await.unwrap();
        let server = AgentAppServer::launch(config, root.path().into(), Some(&path))
            .await
            .unwrap();
        let mut models = server.config_builder_snapshot().await.models;
        models.get_mut("main").unwrap().reasoning.summary = false;
        server
            .set_profile_config(SetConfigBuilderRequest {
                models: Some(models),
                ..Default::default()
            })
            .await
            .unwrap();
        let config = AppConfig::load(Some(&path)).await.unwrap();
        assert_eq!(
            config
                .active_model_config()
                .unwrap()
                .reasoning
                .effort
                .as_deref(),
            Some("high")
        );
        assert!(!config.active_model_config().unwrap().reasoning.summary);
        server.shutdown().await;
    }
}

#[tokio::test]
async fn guarded_model_replacement_rejects_a_stale_window_without_losing_new_aliases() {
    let root = tempfile::tempdir().unwrap();
    let (path, first) = launch_named(root.path(), "json").await;
    let config = AppConfig::load(Some(&path)).await.unwrap();
    let second = AgentAppServer::launch(config, root.path().into(), Some(&path))
        .await
        .unwrap();
    let stale = second.config_builder_snapshot().await;
    let mut first_models = stale.models.clone();
    first_models.insert("added-a".into(), stale.models["cheap"].clone());
    let fresh = first
        .set_profile_config(SetConfigBuilderRequest {
            models: Some(first_models),
            expected_models: Some(stale.models.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let history = second.config_history().await.unwrap();
    let mut second_models = stale.models.clone();
    second_models.insert("added-b".into(), stale.models["cheap"].clone());
    let error = second
        .set_profile_config(SetConfigBuilderRequest {
            models: Some(second_models),
            expected_models: Some(stale.models),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("catalogue changed"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(second.config_history().await.unwrap(), history);
    let mut retry = fresh.models.clone();
    retry.insert("added-b".into(), fresh.models["cheap"].clone());
    let result = second
        .set_profile_config(SetConfigBuilderRequest {
            models: Some(retry),
            expected_models: Some(fresh.models),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(result.models.contains_key("added-a") && result.models.contains_key("added-b"));
    first.shutdown().await;
    second.shutdown().await;
}

#[tokio::test]
async fn history_restore_preserves_connections_registered_after_the_revision() {
    use proteus_contracts::app_protocol::http::ComponentPackageInstall;

    let root = tempfile::tempdir().unwrap();
    let (path, server) = launch_named(root.path(), "json").await;
    let original = server.config_builder_snapshot().await;
    server
        .set_profile_config(SetConfigBuilderRequest {
            permission_mode: Some(PermissionMode::Plan),
            ..Default::default()
        })
        .await
        .unwrap();
    let historical = server.config_history().await.unwrap().revisions[0]
        .state
        .clone();
    assert_eq!(historical.model_connections, ["fake"]);
    server
        .set_profile_config(SetConfigBuilderRequest {
            component_packages: vec![ComponentPackageInstall {
                id: "new-connection".into(),
                command: vec![
                    crate::test_model::reference_module()
                        .to_string_lossy()
                        .into_owned(),
                ],
                env_allowlist: Vec::new(),
                initial_config: BTreeMap::from([(
                    "model".into(),
                    BTreeMap::from([("new-model".into(), json!({"implementation":"fake"}))]),
                )]),
                exports: BTreeMap::from([("model".into(), vec!["new-model".into()])]),
                replace: Vec::new(),
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    let current = server.config_builder_snapshot().await;
    let mut restore_config = historical.module_config.clone();
    // The client restores managed fields, retaining seeds of components that
    // were registered after this revision because component registration survives.
    for (id, config) in &current.module_config["model"] {
        if !historical.model_connections.contains(id) {
            restore_config
                .entry("model".into())
                .or_default()
                .insert(id.clone(), config.clone());
        }
    }
    let restored = server
        .set_profile_config(SetConfigBuilderRequest {
            model: Some(historical.model),
            models: Some(historical.models),
            role_models: Some(historical.role_models),
            permission_mode: Some(PermissionMode::Normal),
            module_config: restore_config,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(restored.permission_mode, "normal");
    assert_eq!(restored.model, original.model);
    assert_eq!(restored.models, original.models);
    assert_eq!(
        restored.module_config["model"]["new-model"],
        json!({"implementation":"fake"})
    );
    assert!(
        restored
            .plugins
            .iter()
            .any(|plugin| plugin.id == "new-connection")
    );
    let loaded = AppConfig::load(Some(&path)).await.unwrap();
    assert_eq!(loaded.module_config, restored.module_config);
    assert_eq!(
        super::super::config_builder_state(&loaded).model_connections,
        ["fake", "new-model"]
    );
    server.shutdown().await;
}
