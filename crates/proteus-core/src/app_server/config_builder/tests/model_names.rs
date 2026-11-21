use super::*;

fn source() -> Value {
    serde_json::json!({
        "model": "main",
        "models": {
            "main": {"provider": "fake", "model": "large", "effort": "high"},
            "budget": {"provider": "fake", "model": "small", "effort": "low"}
        },
        "module_config": {"model": {"fake": {"implementation": "fake"}}},
        "agent_control": {"roles": [
            {"name": "inherit", "description": "inherits current model", "config": {"tools": {"enabled": ["read_file"]}}},
            {"name": "fixed", "description": "uses alias", "model": "budget", "config": {}}
        ]}
    })
}

async fn write(path: &Path, value: &Value) {
    let text = if path.extension().unwrap() == "json" {
        serde_json::to_string_pretty(value).unwrap()
    } else {
        format!(
            "# model catalogue owner\n{}",
            toml::to_string_pretty(value).unwrap()
        )
    };
    tokio::fs::write(path, text).await.unwrap();
}

async fn read(path: &Path) -> Value {
    let text = tokio::fs::read_to_string(path).await.unwrap();
    if path.extension().unwrap() == "json" {
        serde_json::from_str(&text).unwrap()
    } else {
        serde_json::to_value(toml::from_str::<toml::Value>(&text).unwrap()).unwrap()
    }
}

#[tokio::test]
async fn saving_named_model_source_preserves_one_catalogue_and_reloadable_aliases() {
    for extension in ["toml", "json"] {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(format!("agent.{extension}"));
        let initial = source();
        write(&path, &initial).await;
        let mut config = AppConfig::load(Some(&path)).await.unwrap();
        config.permissions.mode = PermissionMode::Auto;
        config.active_provider = Some("budget".into());
        persist_config_builder(&path, &config).await.unwrap();
        let saved = read(&path).await;
        assert_eq!(saved["model"], "budget");
        assert_eq!(saved["models"], initial["models"]);
        assert!(saved.get("providers").is_none() && saved.get("active_provider").is_none());
        let mut loaded = AppConfig::load(Some(&path)).await.unwrap();
        assert_eq!(loaded.active_model_config().unwrap().model, "small");
        loaded.providers.get_mut("budget").unwrap().reasoning.effort = Some("high".into());
        persist_config_builder(&path, &loaded).await.unwrap();
        let saved = read(&path).await;
        assert!(saved.get("providers").is_none());
        assert_eq!(
            saved["models"]["budget"]["effort"], "high",
            "retain the authored flat spelling"
        );
        assert_eq!(
            saved["agent_control"]["roles"],
            initial["agent_control"]["roles"]
        );
        let reloaded = AppConfig::load(Some(&path)).await.unwrap();
        assert_eq!(
            reloaded
                .active_model_config()
                .unwrap()
                .reasoning
                .effort
                .as_deref(),
            Some("high")
        );
    }
}

#[tokio::test]
async fn unrelated_inline_child_save_preserves_inheritance_and_outer_fixed_alias() {
    for extension in ["toml", "json"] {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(format!("agent.{extension}"));
        let initial = source();
        write(&path, &initial).await;
        for role in ["inherit", "fixed"] {
            let reference = crate::core::config_reference(&path, role).unwrap();
            let mut config = AppConfig::load(Some(&reference)).await.unwrap();
            config.permissions.mode = PermissionMode::Auto;
            persist_config_builder(&reference, &config).await.unwrap();
        }
        let saved = read(&path).await;
        assert_eq!(saved["models"], initial["models"]);
        assert_eq!(saved["model"], "main");
        let inherited = &saved["agent_control"]["roles"][0]["config"];
        assert!(inherited.get("model").is_none() && inherited.get("active_provider").is_none());
        assert!(inherited.get("models").is_none() && inherited.get("providers").is_none());
        assert!(
            inherited
                .get("module_config")
                .and_then(|settings| settings.get("model"))
                .is_none()
        );
        assert_eq!(saved["agent_control"]["roles"][1]["model"], "budget");
        let root = AppConfig::load(Some(&path)).await.unwrap();
        assert!(root.agent_control.roles[0].model.is_none());
        assert_eq!(
            root.agent_control.roles[1].model.as_ref().unwrap().model,
            "small"
        );
    }
}

#[tokio::test]
async fn deliberate_child_model_edit_keeps_one_explicit_choice_and_can_return_to_inheritance() {
    for extension in ["toml", "json"] {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(format!("agent.{extension}"));
        write(&path, &source()).await;
        let reference = crate::core::config_reference(&path, "fixed").unwrap();
        let mut child = AppConfig::load(Some(&reference)).await.unwrap();
        child.active_provider = Some("main".into());
        persist_config_builder(&reference, &child).await.unwrap();
        let saved = read(&path).await;
        let role = &saved["agent_control"]["roles"][1];
        assert!(role.get("model").is_none());
        assert_eq!(role["config"]["model"], "main");
        let loaded = AppConfig::load(Some(&path)).await.unwrap();
        assert_eq!(
            loaded.agent_control.roles[1].model.as_ref().unwrap().model,
            "large"
        );
        let mut child = AppConfig::load(Some(&reference)).await.unwrap();
        child.active_provider = None;
        persist_config_builder(&reference, &child).await.unwrap();
        let loaded = AppConfig::load(Some(&path)).await.unwrap();
        assert!(loaded.agent_control.roles[1].model.is_none());
    }
}

#[tokio::test]
async fn directory_overlay_keeps_named_selection_without_freezing_the_shared_catalogue() {
    let home = tempfile::tempdir().unwrap();
    let base = home.path().join("base.toml");
    let mut initial = source();
    initial.as_object_mut().unwrap().remove("agent_control");
    write(&base, &initial).await;
    let target = config_builder_target_path(Some(home.path())).unwrap();
    for model in ["updated-once", "updated-twice"] {
        let config = AppConfig::load(Some(home.path())).await.unwrap();
        persist_config_builder(&target, &config).await.unwrap();
        let overlay = read(&target).await;
        assert!(overlay.get("models").is_none() && overlay.get("providers").is_none());
        initial["models"]["main"]["model"] = model.into();
        write(&base, &initial).await;
        let loaded = AppConfig::load(Some(home.path())).await.unwrap();
        assert_eq!(loaded.active_provider.as_deref(), Some("main"));
        assert_eq!(loaded.active_model_config().unwrap().model, model);
    }
}

// File includes retain their choice and catalogue spellings independently.
#[tokio::test]
async fn file_include_model_edits_remain_reloadable_without_rewriting_base() {
    for extension in ["toml", "json"] {
        for choice_key in ["model", "active_provider"] {
            for catalogue_key in ["models", "providers"] {
                let home = tempfile::tempdir().unwrap();
                let base_name = format!("base.{extension}");
                let base = home.path().join(&base_name);
                let wrapper = home.path().join(format!("agent.{extension}"));
                let mut initial = serde_json::json!({
                    "module_config": {"model": {"fake": {"implementation": "fake"}}}
                });
                initial[choice_key] = "main".into();
                initial[catalogue_key] = serde_json::json!({
                    "main": {"provider": "fake", "model": "large"},
                    "budget": {"provider": "fake", "model": "small"}
                });
                if catalogue_key == "models" {
                    initial[catalogue_key]["main"]["effort"] = "high".into();
                } else {
                    initial[catalogue_key]["main"]["reasoning"] =
                        serde_json::json!({"effort": "high"});
                }
                write(&base, &initial).await;
                write(&wrapper, &serde_json::json!({"include": base_name})).await;
                let original_base = tokio::fs::read(&base).await.unwrap();
                for (index, expected) in ["main", "budget", "main"].into_iter().enumerate() {
                    let mut config = AppConfig::load(Some(&wrapper)).await.unwrap();
                    config.permissions.mode = PermissionMode::Auto;
                    config.active_provider = Some(expected.into());
                    persist_config_builder(&wrapper, &config).await.unwrap();
                    let loaded = AppConfig::load(Some(&wrapper))
                        .await
                        .unwrap_or_else(|error| {
                            panic!("reload {extension}/{choice_key}/{catalogue_key}: {error:#}")
                        });
                    assert_eq!(loaded.active_provider.as_deref(), Some(expected));
                    assert_eq!(tokio::fs::read(&base).await.unwrap(), original_base);
                    let saved = read(&wrapper).await;
                    assert_eq!(saved["include"], base_name);
                    assert!(saved.get("models").is_none() && saved.get("providers").is_none());
                    if index == 0 {
                        assert!(saved.get("model").is_none());
                        assert!(saved.get("active_provider").is_none());
                    }
                }
                let saved_bytes = tokio::fs::read(&wrapper).await.unwrap();
                let mut config = AppConfig::load(Some(&wrapper)).await.unwrap();
                config.active_provider = None;
                assert!(persist_config_builder(&wrapper, &config).await.is_err());
                assert_eq!(tokio::fs::read(&wrapper).await.unwrap(), saved_bytes);
                let mut config = AppConfig::load(Some(&wrapper)).await.unwrap();
                config.providers.get_mut("main").unwrap().model = "changed".into();
                assert!(persist_config_builder(&wrapper, &config).await.is_err());
                assert_eq!(tokio::fs::read(&wrapper).await.unwrap(), saved_bytes);
                assert_eq!(tokio::fs::read(&base).await.unwrap(), original_base);
            }
        }
    }
}

#[tokio::test]
async fn local_model_choice_over_an_unselected_include_can_be_cleared() {
    for extension in ["toml", "json"] {
        for choice_key in ["model", "active_provider"] {
            for catalogue_key in ["models", "providers"] {
                for included_null in [false, true] {
                    if included_null && extension == "toml" {
                        continue;
                    }
                    let home = tempfile::tempdir().unwrap();
                    let base_name = format!("base.{extension}");
                    let base = home.path().join(&base_name);
                    let wrapper = home.path().join(format!("agent.{extension}"));
                    let mut initial = serde_json::json!({
                        "module_config": {"model": {"fake": {"implementation": "fake"}}}
                    });
                    initial[catalogue_key] = serde_json::json!({
                        "main": {"provider": "fake", "model": "large"},
                        "budget": {"provider": "fake", "model": "small"}
                    });
                    if included_null {
                        initial[choice_key] = Value::Null;
                    }
                    write(&base, &initial).await;
                    let mut local = serde_json::json!({"include": base_name});
                    local[choice_key] = "main".into();
                    write(&wrapper, &local).await;
                    let original_base = tokio::fs::read(&base).await.unwrap();
                    for expected in [Some("main"), Some("budget"), None] {
                        let mut config = AppConfig::load(Some(&wrapper)).await.unwrap();
                        config.permissions.mode = PermissionMode::Auto;
                        config.active_provider = expected.map(str::to_owned);
                        persist_config_builder(&wrapper, &config).await.unwrap();
                        let loaded = AppConfig::load(Some(&wrapper)).await.unwrap();
                        assert_eq!(loaded.active_provider.as_deref(), expected);
                        assert_eq!(tokio::fs::read(&base).await.unwrap(), original_base);
                        let saved = read(&wrapper).await;
                        assert!(saved.get("models").is_none() && saved.get("providers").is_none());
                        if expected.is_none() {
                            assert!(saved.get("model").is_none());
                            assert!(saved.get("active_provider").is_none());
                        }
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn inline_owned_legacy_catalogue_edit_does_not_require_pinning_a_choice() {
    for extension in ["toml", "json"] {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(format!("agent.{extension}"));
        let mut initial = source();
        let mut providers = initial["models"].clone();
        for preset in providers.as_object_mut().unwrap().values_mut() {
            let effort = preset.as_object_mut().unwrap().remove("effort").unwrap();
            preset["reasoning"] = serde_json::json!({"effort":effort,"summary":true});
        }
        initial["agent_control"]["roles"][0]["config"]["providers"] = providers;
        write(&path, &initial).await;
        let reference = crate::core::config_reference(&path, "inherit").unwrap();
        let mut child = AppConfig::load(Some(&reference)).await.unwrap();
        assert!(child.model_source.catalogue_writable);
        child.providers.get_mut("budget").unwrap().model = "new-small".into();
        persist_config_builder(&reference, &child).await.unwrap();
        let saved = read(&path).await;
        let config = &saved["agent_control"]["roles"][0]["config"];
        assert_eq!(config["providers"]["budget"]["model"], "new-small");
        assert!(config.get("model").is_none());
        assert!(config.get("active_provider").is_none());
        let loaded = AppConfig::load(Some(&reference)).await.unwrap();
        assert_eq!(loaded.providers["budget"].model, "new-small");
        assert_eq!(saved["models"], initial["models"]);
    }
}

#[tokio::test]
async fn unrelated_inline_save_preserves_partial_local_model_settings() {
    let mut failures = Vec::new();
    for extension in ["toml", "json"] {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(format!("agent.{extension}"));
        let mut initial = source();
        initial["models"]["other"] = serde_json::json!({"provider": "other", "model": "second"});
        initial["module_config"]["model"]["other"] = serde_json::json!({"marker": "parent-before"});
        initial["agent_control"]["roles"][0]["config"]["module_config"] =
            serde_json::json!({"model": {"fake": {"implementation": "child-local"}}});
        write(&path, &initial).await;
        if extension == "toml" {
            let authored = tokio::fs::read_to_string(&path).await.unwrap().replace(
                "implementation = \"child-local\"",
                "# Authored child-only transport override\nimplementation = \"child-local\"",
            );
            tokio::fs::write(&path, authored).await.unwrap();
        }
        let reference = crate::core::config_reference(&path, "inherit").unwrap();
        let mut child = AppConfig::load(Some(&reference)).await.unwrap();
        assert_eq!(
            child.module_config["model"]["other"]["marker"],
            "parent-before"
        );
        child.tools.enabled = vec!["read_file".into(), "list_dir".into()];
        persist_config_builder(&reference, &child).await.unwrap();
        let mut saved = read(&path).await;
        let local = &saved["agent_control"]["roles"][0]["config"]["module_config"]["model"];
        assert_eq!(local["fake"]["implementation"], "child-local");
        if local.get("other").is_some() {
            failures.push(format!("{extension} captured inherited settings: {local}"));
        }
        if extension == "toml"
            && !tokio::fs::read_to_string(&path)
                .await
                .unwrap()
                .contains("# Authored child-only transport override")
        {
            failures.push("toml removed the authored local comment".into());
        }
        saved["module_config"]["model"]["other"]["marker"] = "parent-after".into();
        write(&path, &saved).await;
        let mut reloaded = AppConfig::load(Some(&reference)).await.unwrap();
        if reloaded.module_config["model"]["other"]["marker"] != "parent-after" {
            failures.push(format!(
                "{extension} blocked parent update: {}",
                reloaded.module_config["model"]["other"]
            ));
        }
        assert_eq!(
            reloaded.module_config["model"]["fake"]["implementation"],
            "child-local"
        );
        assert_eq!(reloaded.tools.enabled, ["read_file", "list_dir"]);

        // An actual transport edit still creates the requested child override.
        reloaded
            .module_config
            .get_mut("model")
            .unwrap()
            .get_mut("other")
            .unwrap()["marker"] = "child-edited".into();
        persist_config_builder(&reference, &reloaded).await.unwrap();
        let mut saved = read(&path).await;
        assert_eq!(
            saved["agent_control"]["roles"][0]["config"]["module_config"]["model"]["other"]["marker"],
            "child-edited"
        );
        saved["module_config"]["model"]["other"]["marker"] = "parent-later".into();
        write(&path, &saved).await;
        let reloaded = AppConfig::load(Some(&reference)).await.unwrap();
        assert_eq!(
            reloaded.module_config["model"]["other"]["marker"],
            "child-edited"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
