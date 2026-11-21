//! Persistence/load boundaries only; these fixtures do not launch a model.

use super::*;
use serde_json::json;

async fn write_document(path: &Path, value: &Value) {
    let text = if path
        .extension()
        .is_some_and(|extension| extension == "json")
    {
        serde_json::to_string_pretty(value).unwrap()
    } else {
        toml::to_string_pretty(value).unwrap()
    };
    tokio::fs::write(path, text).await.unwrap();
}

async fn read_document(path: &Path) -> Value {
    let text = tokio::fs::read_to_string(path).await.unwrap();
    if path
        .extension()
        .is_some_and(|extension| extension == "json")
    {
        serde_json::from_str(&text).unwrap()
    } else {
        serde_json::to_value(toml::from_str::<toml::Value>(&text).unwrap()).unwrap()
    }
}

fn provider_fragment(model: &str) -> Value {
    json!({"providers": {"demo": {"provider": "synthetic", "model": model}}})
}

#[tokio::test]
async fn existing_include_profiles_preserve_provider_absence_and_explicit_overrides() {
    for extension in ["toml", "json"] {
        for local_override in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let fragment_name = format!("providers.{extension}");
            let fragment = dir.path().join(&fragment_name);
            let profile = dir.path().join(format!("profile.{extension}"));
            write_document(&fragment, &provider_fragment("inherited-v1")).await;
            let mut initial = json!({
                "include": fragment_name,
                "active_provider": "demo",
                "runtime": {"model_timeout_ms": 98765}
            });
            if local_override {
                initial["providers"] = json!({"demo": {"model": "local-v1"}});
            }
            write_document(&profile, &initial).await;
            let mut config = AppConfig::load(Some(&profile)).await.unwrap();
            assert_eq!(
                config.providers["demo"].model,
                if local_override {
                    "local-v1"
                } else {
                    "inherited-v1"
                }
            );
            config.tools.enabled = vec!["managed-fixture-tool".into()];

            persist_config_builder(&profile, &config).await.unwrap();

            let saved = read_document(&profile).await;
            assert_eq!(saved.get("providers"), initial.get("providers"));
            assert_eq!(saved["include"], initial["include"]);
            assert_eq!(saved["runtime"], initial["runtime"]);
            write_document(&fragment, &provider_fragment("inherited-v2")).await;
            let reloaded = AppConfig::load(Some(&profile)).await.unwrap();
            assert_eq!(
                reloaded.providers["demo"].model,
                if local_override {
                    "local-v1"
                } else {
                    "inherited-v2"
                }
            );
            assert_eq!(reloaded.active_provider.as_deref(), Some("demo"));
            assert_eq!(reloaded.tools.enabled, config.tools.enabled);
        }
    }
}

#[tokio::test]
async fn directory_first_and_repeated_overlay_saves_do_not_freeze_providers() {
    let dir = tempfile::tempdir().unwrap();
    let fragment = dir.path().join("base.toml");
    let mut initial = provider_fragment("inherited-v1");
    initial["active_provider"] = json!("demo");
    write_document(&fragment, &initial).await;
    let target = config_builder_target_path(Some(dir.path())).unwrap();
    assert!(!target.exists());
    for next_model in ["inherited-v2", "inherited-v3"] {
        let mut config = AppConfig::load(Some(dir.path())).await.unwrap();
        config.tools.enabled = vec!["managed-fixture-tool".into()];

        persist_config_builder(&target, &config).await.unwrap();

        assert!(read_document(&target).await.get("providers").is_none());
        initial["providers"]["demo"]["model"] = json!(next_model);
        write_document(&fragment, &initial).await;
        let reloaded = AppConfig::load(Some(dir.path())).await.unwrap();
        assert_eq!(reloaded.providers["demo"].model, next_model);
        assert_eq!(reloaded.active_provider.as_deref(), Some("demo"));
        assert_eq!(reloaded.tools.enabled, config.tools.enabled);
    }
}

#[tokio::test]
async fn absent_standalone_profiles_still_seed_the_selected_provider() {
    for extension in ["toml", "json"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("fresh.{extension}"));
        let mut config = AppConfig::default();
        let active = config.active_provider.clone().unwrap();
        config.providers.get_mut(&active).unwrap().model = "standalone-fixture".into();
        assert!(!path.exists());

        persist_config_builder(&path, &config).await.unwrap();

        assert!(read_document(&path).await.get("providers").is_some());
        let reloaded = AppConfig::load(Some(&path)).await.unwrap();
        assert_eq!(reloaded.active_provider, config.active_provider);
        assert_eq!(
            serde_json::to_value(&reloaded.providers).unwrap(),
            serde_json::to_value(&config.providers).unwrap()
        );
        assert_eq!(reloaded.providers[&active].model, "standalone-fixture");
    }
}
