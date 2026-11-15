use super::*;

async fn write_profile(path: &Path, source: &Value) {
    let text = if path
        .extension()
        .is_some_and(|extension| extension == "json")
    {
        serde_json::to_string_pretty(source).unwrap()
    } else {
        toml::to_string_pretty(source).unwrap()
    };
    tokio::fs::write(path, text).await.unwrap();
}

async fn read_profile(path: &Path) -> Value {
    let text = tokio::fs::read_to_string(path).await.unwrap();
    if path
        .extension()
        .is_some_and(|extension| extension == "json")
    {
        serde_json::from_str(&text).unwrap()
    } else {
        let value: toml::Value = toml::from_str(&text).unwrap();
        serde_json::to_value(value).unwrap()
    }
}

#[tokio::test]
async fn missing_standalone_profile_preserves_instruction_sources() {
    for extension in ["toml", "json"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("profile.{extension}"));
        let relative = dir.path().join("relative.md");
        let absolute = dir.path().join("absolute.md");
        tokio::fs::write(&relative, "relative v1").await.unwrap();
        tokio::fs::write(&absolute, "absolute v1").await.unwrap();
        let instructions = serde_json::json!([
            {"kind": "System", "file": "relative.md", "priority": 100},
            {"kind": "Developer", "text": "inline text", "priority": 90},
            {"kind": "System", "file": absolute, "priority": 80}
        ]);
        write_profile(&path, &serde_json::json!({"instructions": instructions})).await;
        let mut config = AppConfig::load(Some(&path)).await.unwrap();
        let runtime_instructions = serde_json::to_value(&config.instructions).unwrap();
        assert_eq!(config.instructions[0].text.as_deref(), Some("relative v1"));
        assert_eq!(config.instructions[2].text.as_deref(), Some("absolute v1"));

        config.tools.enabled = vec!["managed-change".into()];
        persist_config_builder(&path, &config).await.unwrap();
        assert_eq!(read_profile(&path).await["instructions"], instructions);

        tokio::fs::remove_file(&path).await.unwrap();
        persist_config_builder(&path, &config).await.unwrap();
        assert_eq!(read_profile(&path).await["instructions"], instructions);
        assert_eq!(
            serde_json::to_value(&config.instructions).unwrap(),
            runtime_instructions
        );
        let loaded = AppConfig::load(Some(&path)).await.unwrap();
        assert_eq!(
            serde_json::to_value(&loaded.instructions).unwrap(),
            runtime_instructions
        );
        assert_eq!(loaded.tools.enabled, config.tools.enabled);

        tokio::fs::write(&relative, "relative v2").await.unwrap();
        tokio::fs::write(&absolute, "absolute v2").await.unwrap();
        let loaded = AppConfig::load(Some(&path)).await.unwrap();
        assert_eq!(loaded.instructions[0].text.as_deref(), Some("relative v2"));
        assert_eq!(loaded.instructions[1].text.as_deref(), Some("inline text"));
        assert_eq!(loaded.instructions[2].text.as_deref(), Some("absolute v2"));
    }
}

#[tokio::test]
async fn bootstrap_does_not_substitute_cached_text_for_missing_instruction_assets() {
    for extension in ["toml", "json"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("profile.{extension}"));
        let asset = dir.path().join("rules.md");
        tokio::fs::write(&asset, "cached text").await.unwrap();
        let source = serde_json::json!({"instructions": [
            {"kind": "System", "file": "rules.md", "priority": 100}
        ]});
        write_profile(&path, &source).await;
        let config = AppConfig::load(Some(&path)).await.unwrap();
        tokio::fs::remove_file(&path).await.unwrap();
        tokio::fs::remove_file(&asset).await.unwrap();
        persist_config_builder(&path, &config).await.unwrap();
        assert_eq!(
            read_profile(&path).await["instructions"],
            source["instructions"]
        );
        let error = AppConfig::load(Some(&path)).await.unwrap_err();
        assert!(format!("{error:#}").contains("failed to read instructions file"));
        assert_eq!(config.instructions[0].text.as_deref(), Some("cached text"));
    }
}

#[tokio::test]
async fn directory_overlay_does_not_materialize_resolved_instructions() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("base.toml");
    let asset = dir.path().join("rules.md");
    let instructions = serde_json::json!([
        {"kind": "System", "file": "rules.md", "priority": 100}
    ]);
    write_profile(&source, &serde_json::json!({"instructions": instructions})).await;
    tokio::fs::write(&asset, "v1").await.unwrap();
    let target = config_builder_target_path(Some(dir.path())).unwrap();
    for text in ["v2", "v3"] {
        let mut config = AppConfig::load(Some(dir.path())).await.unwrap();
        config.tools.enabled = vec![text.into()];
        persist_config_builder(&target, &config).await.unwrap();
        assert!(read_profile(&target).await.get("instructions").is_none());
        assert_eq!(read_profile(&source).await["instructions"], instructions);
        tokio::fs::write(&asset, text).await.unwrap();
        let loaded = AppConfig::load(Some(dir.path())).await.unwrap();
        assert_eq!(loaded.instructions[0].text.as_deref(), Some(text));
        assert_eq!(loaded.tools.enabled, config.tools.enabled);
    }
}
