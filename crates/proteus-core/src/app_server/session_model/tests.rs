use super::*;
use crate::{
    app_server::AgentAppServer, contracts::ProcessModelDescriptor, domain::PermissionMode,
    model_standard::ModelCapabilities,
};
use serde_json::json;
use std::path::Path;

fn config() -> AppConfig {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/process_model.py");
    let settings = |provider: &str, limit| {
        json!({
            "descriptor": ProcessModelDescriptor { adapter_id: format!("adapter-{provider}"),
                capabilities: ModelCapabilities::basic_text_and_tools().with_streaming(true)
                    .with_reasoning_config(true).with_max_input_tokens(Some(limit)), hosted_tools: vec![] },
            "features": ["model.catalog.v1"],
            "catalog": {"models": (["default", "other"].map(|suffix| json!({
                "id": format!("{provider}-{suffix}"), "display_name": suffix,
                "description": null, "hidden": false, "reasoning_efforts": ["low", "high"],
                "default_reasoning_effort": "low"
            })))}
        })
    };
    let mut config = serde_json::from_value(json!({
        "active_provider": "main", "providers": {
            "main": {"provider": "first", "model": "first-default", "reasoning": {"effort": "low"}},
            "cheap": {"provider": "second", "model": "second-default", "reasoning": {"effort": "high"}}
        },
        "components": {"model-worker": {"command": "python3", "args": ["-B", fixture],
            "exports": {"model": {"first": {}, "second": {}}}}},
        "module_config": {"model": {"first": settings("first", 111), "second": settings("second", 222)}}
    })).unwrap();
    crate::test_support::storage_config(&mut config);
    config
}

fn reasoning() -> ReasoningConfig {
    ReasoningConfig::new(Some("high".into()), true).with_budget_tokens(Some(73))
}

async fn launch(config: AppConfig, root: &Path, cwd: &Path) -> AppServerHandle {
    let path = root.join("agent.json");
    if !path.exists() {
        std::fs::write(&path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    }
    AgentAppServer::launch(config, cwd.into(), Some(&path))
        .await
        .unwrap()
}

fn last_files(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut files = std::fs::read_dir(root.join("model-selections"))
        .into_iter()
        .flatten()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file())
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

#[tokio::test]
async fn unchanged_exact_selection_keeps_the_live_adapter_and_epoch() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let marker = root.path().join("model-pids");
    let mut baseline = config();
    baseline
        .module_config
        .get_mut("model")
        .unwrap()
        .get_mut("first")
        .unwrap()["pid_marker"] = json!(marker);
    let server = launch(baseline, root.path(), cwd.path()).await;
    let epoch = server.runtime.configuration_view().await.0.epoch;
    let pids = std::fs::read_to_string(&marker).unwrap();
    let model = server.runtime.model_ref().await.unwrap();
    let reasoning = server.runtime.reasoning().await;
    server
        .apply_model_configuration(model, reasoning, true)
        .await
        .unwrap();
    assert_eq!(server.runtime.configuration_view().await.0.epoch, epoch);
    assert_eq!(std::fs::read_to_string(marker).unwrap(), pids);
    assert!(
        server
            .runtime
            .session_store()
            .unwrap()
            .read_metadata_document(FILE)
            .await
            .unwrap()
            .is_some()
    );
    server.shutdown().await;
}

#[tokio::test]
async fn another_model_on_the_same_bound_route_reuses_worker_and_refreshes_exact_reasoning() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let marker = root.path().join("model-pids");
    let mut baseline = config();
    baseline
        .module_config
        .get_mut("model")
        .unwrap()
        .get_mut("first")
        .unwrap()["pid_marker"] = json!(marker);
    let server = launch(baseline, root.path(), cwd.path()).await;
    let epoch = server.runtime.configuration_view().await.0.epoch;
    let pids = std::fs::read_to_string(&marker).unwrap();
    server
        .apply_model_configuration(ModelRef::new("first", "first-other"), reasoning(), true)
        .await
        .unwrap();
    assert_eq!(server.runtime.configuration_view().await.0.epoch, epoch);
    assert_eq!(std::fs::read_to_string(marker).unwrap(), pids);
    assert_eq!(
        server.runtime.model_ref().await,
        Some(ModelRef::new("first", "first-other"))
    );
    assert_eq!(server.runtime.reasoning().await, reasoning());
    assert_eq!(
        server
            .runtime
            .configuration_view()
            .await
            .0
            .registry
            .model_config
            .as_ref()
            .unwrap()
            .model,
        "first-other"
    );
    server.set_reasoning_enabled(false).await.unwrap();
    server.set_reasoning_enabled(true).await.unwrap();
    assert_eq!(server.runtime.reasoning().await, reasoning());
    server.shutdown().await;
}

#[tokio::test]
async fn picker_discovers_both_routes_and_switches_an_identical_model_id() {
    use proteus_contracts::app_protocol::{
        config::ConfigSummary,
        settings::{SessionSettingControl, SessionSettingUpdate},
    };

    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let marker = root.path().join("inactive-catalog-started");
    let release = root.path().join("inactive-catalog-release");
    let mut baseline = config();
    let routes = baseline.module_config.get_mut("model").unwrap();
    for provider in ["first", "second"] {
        routes.get_mut(provider).unwrap()["catalog"]["models"][1]["id"] = json!("shared");
    }
    let second = routes.get_mut("second").unwrap();
    second["catalog_wait"] = json!(true);
    second["catalog_marker"] = json!(marker);
    second["catalog_release"] = json!(release);
    let server = launch(baseline.clone(), root.path(), cwd.path()).await;
    let original = std::fs::read(root.path().join("agent.json")).unwrap();
    let local: ConfigSummary = serde_json::from_value(server.local_config_summary().await).unwrap();
    assert_eq!(
        local
            .model_options
            .iter()
            .map(|option| (option.provider.as_str(), option.name.as_str()))
            .collect::<Vec<_>>(),
        [("first", "first-default"), ("second", "second-default")]
    );
    assert!(!marker.exists());

    std::fs::write(release, "ready").unwrap();
    let discovered: ConfigSummary = serde_json::from_value(server.config_summary().await).unwrap();
    assert!(marker.exists());
    assert_eq!(
        discovered
            .model_options
            .iter()
            .filter(|option| option.name == "shared")
            .map(|option| option.provider.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    let settings = server.session_settings().await;
    let SessionSettingControl::Select { options, .. } = &settings
        .iter()
        .find(|setting| setting.id == "model")
        .unwrap()
        .control
    else {
        panic!("model setting must be a selector");
    };
    assert_eq!(
        options
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>(),
        ["first-default", "shared"],
        "the string selector remains scoped to its current provider"
    );

    let setting = |value| SessionSettingUpdate {
        id: "model".into(),
        value,
    };
    server
        .set_session_setting(setting(json!("shared")))
        .await
        .unwrap();
    assert_eq!(
        server.runtime.model_ref().await,
        Some(ModelRef::new("first", "shared"))
    );
    assert_eq!(
        server.runtime.reasoning().await.effort.as_deref(),
        Some("low")
    );
    let defaults = last_files(root.path());
    server
        .set_session_setting(setting(json!({"provider": "second", "model": "shared"})))
        .await
        .unwrap();
    assert_eq!(
        server.runtime.model_ref().await,
        Some(ModelRef::new("second", "shared"))
    );
    assert_eq!(server.runtime.reasoning().await, ReasoningConfig::default());
    assert_eq!(last_files(root.path()), defaults);
    assert_eq!(
        std::fs::read(root.path().join("agent.json")).unwrap(),
        original
    );
    let directory = server.session_dir_path().unwrap();
    server.shutdown().await;
    drop(server);
    let resumed = AgentAppServer::launch_resumed(
        baseline,
        cwd.path().into(),
        Some(&root.path().join("agent.json")),
        directory,
    )
    .await
    .unwrap();
    assert_eq!(
        resumed.runtime.model_ref().await,
        Some(ModelRef::new("second", "shared"))
    );
    resumed.shutdown().await;
}

#[tokio::test]
async fn no_catalog_uses_all_aliases_capability_metadata_for_the_same_physical_model() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut baseline = config();
    let mut budget = baseline.providers["main"].clone();
    budget.reasoning_efforts = vec!["low".into()];
    baseline.providers.insert("budget".into(), budget);
    baseline
        .providers
        .get_mut("main")
        .unwrap()
        .reasoning_efforts = vec!["high".into()];
    let settings = baseline
        .module_config
        .get_mut("model")
        .unwrap()
        .get_mut("first")
        .unwrap();
    settings["features"] = json!([]);
    settings.as_object_mut().unwrap().remove("catalog");
    let server = launch(baseline, root.path(), cwd.path()).await;
    let model = ModelRef::new("first", "first-default");
    for effort in [Some("high"), Some("low"), None] {
        server
            .apply_model_configuration(
                model.clone(),
                ReasoningConfig::new(effort.map(str::to_owned), false),
                true,
            )
            .await
            .unwrap();
    }
    assert!(
        server
            .apply_model_configuration(
                model,
                ReasoningConfig::new(Some("unsupported".into()), false),
                true
            )
            .await
            .is_err()
    );
    assert_eq!(server.runtime.reasoning().await.effort, None);
    server.shutdown().await;
}

#[tokio::test]
async fn exact_configuration_rebinds_provider_and_preserves_source_and_permission() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let baseline = config();
    let server = launch(baseline.clone(), root.path(), cwd.path()).await;
    let original = std::fs::read(root.path().join("agent.json")).unwrap();
    server
        .set_permission_mode(PermissionMode::Auto)
        .await
        .unwrap();
    server
        .apply_model_configuration(ModelRef::new("second", "second-other"), reasoning(), false)
        .await
        .unwrap();
    assert_eq!(
        server.runtime.model_ref().await,
        Some(ModelRef::new("second", "second-other"))
    );
    assert_eq!(server.runtime.reasoning().await, reasoning());
    assert_eq!(server.permission_mode().await, PermissionMode::Auto);
    let snapshot = server.runtime.configuration_view().await.0;
    assert_eq!(
        snapshot.registry.model_config.as_ref().unwrap().provider,
        "second"
    );
    assert!(!snapshot.registry.model_config.as_ref().unwrap().stream);
    assert_eq!(
        snapshot
            .registry
            .model_capabilities(&ModelRef::new("second", "second-other"))
            .unwrap()
            .unwrap()
            .max_input_tokens,
        Some(222)
    );
    assert!(
        server
            .runtime
            .model_catalog()
            .await
            .unwrap()
            .unwrap()
            .models
            .iter()
            .all(|entry| entry.id.starts_with("second-"))
    );
    assert_eq!(
        serde_json::to_value(&*server.config.read().await).unwrap(),
        serde_json::to_value(baseline).unwrap()
    );
    assert_eq!(
        std::fs::read(root.path().join("agent.json")).unwrap(),
        original
    );
    assert!(last_files(root.path()).is_empty());
    server.shutdown().await;
}

#[tokio::test]
async fn scoped_choice_survives_cold_resume_without_changing_another_new_session_default() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let baseline = config();
    let server = launch(baseline.clone(), root.path(), cwd.path()).await;
    server.set_model_name("first-other".into()).await.unwrap();
    let defaults = last_files(root.path());
    assert_eq!(defaults.len(), 1);
    server
        .apply_model_configuration(ModelRef::new("second", "second-other"), reasoning(), false)
        .await
        .unwrap();
    server
        .set_model_name("second-default".into())
        .await
        .unwrap();
    server
        .set_reasoning_effort(Some("low".into()))
        .await
        .unwrap();
    assert_eq!(last_files(root.path()), defaults);
    let directory = server.session_dir_path().unwrap();
    server.shutdown().await;
    drop(server);
    let resumed = AgentAppServer::launch_resumed(
        baseline.clone(),
        cwd.path().into(),
        Some(&root.path().join("agent.json")),
        directory,
    )
    .await
    .unwrap();
    assert_eq!(
        resumed.runtime.model_ref().await,
        Some(ModelRef::new("second", "second-default"))
    );
    assert_eq!(
        resumed.runtime.reasoning().await.effort.as_deref(),
        Some("low")
    );
    assert_eq!(resumed.runtime.reasoning().await.budget_tokens, Some(73));
    assert!(
        !resumed
            .runtime
            .configuration_view()
            .await
            .0
            .registry
            .model_config
            .as_ref()
            .unwrap()
            .stream
    );
    resumed.shutdown().await;
    drop(resumed);
    let fresh = launch(baseline, root.path(), cwd.path()).await;
    assert_eq!(
        fresh.runtime.model_ref().await,
        Some(ModelRef::new("first", "first-other"))
    );
    assert_eq!(last_files(root.path()), defaults);
    fresh.shutdown().await;
}

#[tokio::test]
async fn unsupported_model_effort_and_route_leave_the_current_configuration_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let server = launch(config(), root.path(), cwd.path()).await;
    let epoch = server.runtime.configuration_view().await.0.epoch;
    let before = server.runtime.model_ref().await;
    let reasoning_before = server.runtime.reasoning().await;
    for (model, reasoning) in [
        (ModelRef::new("unknown", "unknown"), reasoning()),
        (ModelRef::new("second", "missing"), reasoning()),
        (
            ModelRef::new("second", "second-default"),
            ReasoningConfig::new(Some("ultra".into()), true),
        ),
    ] {
        assert!(
            server
                .apply_model_configuration(model, reasoning, true)
                .await
                .is_err()
        );
        assert_eq!(server.runtime.configuration_view().await.0.epoch, epoch);
        assert_eq!(server.runtime.model_ref().await, before);
        assert_eq!(server.runtime.reasoning().await, reasoning_before);
    }
    assert!(
        server
            .runtime
            .session_store()
            .unwrap()
            .read_metadata_document(FILE)
            .await
            .unwrap()
            .is_none()
    );
    assert!(last_files(root.path()).is_empty());
    server.close_idle_runs().await.unwrap();
    assert!(
        server
            .apply_model_configuration(ModelRef::new("second", "second-other"), reasoning(), false)
            .await
            .is_err()
    );
    server.shutdown().await;
}

#[tokio::test]
async fn profile_refresh_keeps_scoped_binding_until_its_source_route_is_changed() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut baseline = config();
    let server = launch(baseline.clone(), root.path(), cwd.path()).await;
    server
        .apply_model_configuration(ModelRef::new("second", "second-other"), reasoning(), false)
        .await
        .unwrap();
    baseline.runtime.context_timeout_ms += 1;
    let assembly = prepare_assembly(&baseline, cwd.path(), Some(&root.path().join("agent.json")))
        .await
        .unwrap();
    server
        .publish_profile(baseline.clone(), assembly, None, || async { Ok(()) })
        .await
        .unwrap();
    assert_eq!(
        server.runtime.model_ref().await,
        Some(ModelRef::new("second", "second-other"))
    );
    assert_eq!(
        server
            .runtime
            .configuration_view()
            .await
            .0
            .registry
            .model_config
            .as_ref()
            .unwrap()
            .provider,
        "second"
    );
    baseline.providers.get_mut("main").unwrap().model = "first-other".into();
    let assembly = prepare_assembly(&baseline, cwd.path(), Some(&root.path().join("agent.json")))
        .await
        .unwrap();
    server
        .publish_profile(baseline, assembly, None, || async { Ok(()) })
        .await
        .unwrap();
    assert_eq!(
        server.runtime.model_ref().await,
        Some(ModelRef::new("first", "first-other"))
    );
    assert_eq!(
        server
            .runtime
            .configuration_view()
            .await
            .0
            .registry
            .model_config
            .as_ref()
            .unwrap()
            .provider,
        "first"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn builder_model_definitions_remain_saved_when_session_uses_another_route() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let baseline = config();
    let server = launch(baseline.clone(), root.path(), cwd.path()).await;
    server
        .apply_model_configuration(ModelRef::new("second", "second-other"), reasoning(), false)
        .await
        .unwrap();
    let snapshot = server.config_builder_snapshot().await;
    assert_eq!(snapshot.model.as_deref(), Some("main"));
    assert_eq!(snapshot.models["main"].model, "first-default");
    assert_eq!(
        snapshot.models["main"].reasoning,
        baseline.providers["main"].reasoning
    );
    assert_eq!(snapshot.models["cheap"].model, "second-default");
    assert!(
        snapshot
            .model_modules
            .iter()
            .find(|module| module.id == "first")
            .unwrap()
            .active
    );
    assert!(
        !snapshot
            .model_modules
            .iter()
            .find(|module| module.id == "second")
            .unwrap()
            .active
    );
    assert_eq!(snapshot.module_config, baseline.module_config);
    assert_eq!(
        server.runtime.model_ref().await.unwrap(),
        ModelRef::new("second", "second-other")
    );
    server.shutdown().await;
}

#[test]
fn component_icon_does_not_invalidate_scoped_model_binding() {
    let mut config = config();
    let selected = ModelRef::new("second", "second-other");
    let before = binding(&config, &selected).unwrap();
    let component = config.components.get_mut("model-worker").unwrap();
    let mut source = serde_json::to_value(&*component).unwrap();
    source["icon"] = json!("assets/model.svg");
    *component = serde_json::from_value(source).unwrap();
    assert_eq!(binding(&config, &selected).unwrap(), before);
}
