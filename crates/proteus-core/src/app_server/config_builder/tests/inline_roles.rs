use super::*;

fn source() -> Value {
    serde_json::json!({
        "permissions": { "mode": "normal" },
        "runtime": { "model_timeout_ms": 98765 },
        "agent_control": { "max_depth": 2, "roles": [
            { "name": "explore/read", "description": "Reader", "config": {
                "instructions": [{ "kind": "System", "text": "inline reader rules", "priority": 100 }],
                "permissions": { "mode": "plan" },
                "tools": { "enabled": ["read_file"] },
                "runtime": { "context_timeout_ms": 1357 }
            } },
            { "name": "coder", "description": "Writer", "config": {
                "permissions": { "mode": "normal" }, "tools": { "enabled": ["write_file"] }
            } }
        ] }
    })
}

async fn write(path: &Path, source: &Value) {
    let body = if path.extension().unwrap() == "json" {
        serde_json::to_string_pretty(source).unwrap()
    } else {
        format!(
            "# monolithic source comment\n{}",
            toml::to_string_pretty(source).unwrap()
        )
    };
    tokio::fs::write(path, body).await.unwrap();
}

async fn read(path: &Path) -> Value {
    let body = tokio::fs::read_to_string(path).await.unwrap();
    if path.extension().unwrap() == "json" {
        serde_json::from_str(&body).unwrap()
    } else {
        serde_json::to_value(toml::from_str::<toml::Value>(&body).unwrap()).unwrap()
    }
}

#[tokio::test]
async fn saving_root_keeps_inline_role_bodies_instead_of_runtime_references() {
    for extension in ["toml", "json"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("agent.{extension}"));
        let initial = source();
        write(&path, &initial).await;
        let mut root = AppConfig::load(Some(&path)).await.unwrap();
        assert!(
            !crate::core::config_selection(Path::new(&root.agent_control.roles[0].config))
                .unwrap()
                .is_empty()
        );
        root.permissions.mode = PermissionMode::Auto;
        persist_config_builder(&path, &root).await.unwrap();
        let saved = read(&path).await;
        assert_eq!(
            saved["agent_control"]["roles"],
            initial["agent_control"]["roles"]
        );
        assert_eq!(saved["runtime"], initial["runtime"]);
        assert_eq!(
            AppConfig::load(Some(&path)).await.unwrap().permissions.mode,
            PermissionMode::Auto
        );
        if extension == "toml" {
            assert!(
                tokio::fs::read_to_string(&path)
                    .await
                    .unwrap()
                    .contains("# monolithic source comment")
            );
        }
    }
}

#[tokio::test]
async fn saving_inline_role_changes_only_its_source_object_and_reloads_from_same_reference() {
    for extension in ["toml", "json"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("agent.{extension}"));
        let initial = source();
        write(&path, &initial).await;
        let selected = crate::core::config_reference(&path, "explore/read").unwrap();
        let target = config_builder_target_path(Some(&selected)).unwrap();
        assert_eq!(crate::core::physical_config_path(&target), path);
        let mut child = AppConfig::load(Some(&selected)).await.unwrap();
        child.permissions.mode = PermissionMode::Auto;
        child.tools.enabled = vec!["different-tool".into()];
        persist_config_builder(&target, &child).await.unwrap();
        let saved = read(&path).await;
        assert_eq!(saved["permissions"], initial["permissions"]);
        assert_eq!(saved["runtime"], initial["runtime"]);
        assert_eq!(
            saved["agent_control"]["roles"][1],
            initial["agent_control"]["roles"][1]
        );
        let edited = &saved["agent_control"]["roles"][0]["config"];
        assert_eq!(
            edited["instructions"],
            initial["agent_control"]["roles"][0]["config"]["instructions"]
        );
        assert_eq!(
            edited["runtime"],
            initial["agent_control"]["roles"][0]["config"]["runtime"]
        );
        assert_eq!(edited["permissions"]["mode"], "auto");
        let loaded = AppConfig::load(Some(&selected)).await.unwrap();
        assert_eq!(loaded.permissions.mode, PermissionMode::Auto);
        assert_eq!(loaded.tools.enabled, ["different-tool"]);
        assert!(
            !Path::new(&selected).exists(),
            "selector must never become a physical source file"
        );
        if extension == "toml" {
            assert!(
                tokio::fs::read_to_string(&path)
                    .await
                    .unwrap()
                    .contains("# monolithic source comment")
            );
        }
    }
}

#[tokio::test]
async fn removed_monolithic_source_is_not_recreated_with_dangling_role_references() {
    for extension in ["toml", "json"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("agent.{extension}"));
        write(&path, &source()).await;
        let root = AppConfig::load(Some(&path)).await.unwrap();
        let selected = crate::core::config_reference(&path, "coder").unwrap();
        let child = AppConfig::load(Some(&selected)).await.unwrap();
        tokio::fs::remove_file(&path).await.unwrap();
        assert!(persist_config_builder(&path, &root).await.is_err());
        assert!(persist_config_builder(&selected, &child).await.is_err());
        assert!(!path.exists());
    }
}

#[tokio::test]
async fn compact_toml_role_tables_remain_valid_after_nested_managed_settings_are_saved() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent.toml");
    tokio::fs::write(
        &path,
        "# compact source\n[agent_control]\nroles = [{name='reader', description='Reads', config={permissions={mode='plan'}}}]\n",
    )
    .await
    .unwrap();
    let selected = crate::core::config_reference(&path, "reader").unwrap();
    let mut config = AppConfig::load(Some(&selected)).await.unwrap();
    config.permissions.mode = PermissionMode::Auto;
    config.tools.enabled = vec!["new-tool".into()];
    persist_config_builder(&selected, &config).await.unwrap();
    let loaded = AppConfig::load(Some(&selected)).await.unwrap();
    assert_eq!(loaded.permissions.mode, PermissionMode::Auto);
    assert_eq!(loaded.tools.enabled, ["new-tool"]);
    assert!(
        tokio::fs::read_to_string(path)
            .await
            .unwrap()
            .starts_with("# compact source")
    );
}

#[test]
fn root_and_inline_roles_share_one_write_lock_and_store_root() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent.toml");
    let reader = crate::core::config_reference(&path, "explore/read").unwrap();
    let writer = crate::core::config_reference(&path, "coder").unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &path_lock(&path).unwrap(),
        &path_lock(&reader).unwrap()
    ));
    assert!(std::sync::Arc::ptr_eq(
        &path_lock(&path).unwrap(),
        &path_lock(&writer).unwrap()
    ));
    assert_eq!(
        crate::core::config_store_root(&path),
        crate::core::config_store_root(&reader)
    );
}
