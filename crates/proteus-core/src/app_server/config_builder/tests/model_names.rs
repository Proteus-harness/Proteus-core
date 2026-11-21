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
        assert_eq!(saved["models"]["budget"]["reasoning"]["effort"], "high");
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
