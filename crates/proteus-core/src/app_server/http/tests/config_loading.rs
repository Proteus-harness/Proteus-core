use super::*;
use proteus_contracts::{
    app_protocol::config::ConfigSummary, contracts::ProcessModelDescriptor,
    model_standard::ModelCapabilities,
};

#[tokio::test]
async fn local_config_remains_available_while_external_catalog_is_loading() {
    let config_dir = tempfile::tempdir().unwrap();
    let marker = config_dir.path().join("catalog-started");
    let release = config_dir.path().join("catalog-release");
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/process_model.py");
    let config: AppConfig = serde_json::from_value(json!({
        "active_provider": "fixture",
        "providers": {"fixture": {
            "provider": "external", "model": "selected", "reasoning": {"effort": "high"}
        }},
        "components": {"independent": {
            "command": "python3", "args": ["-B", fixture],
            "exports": {"model": {"external": {"timeout_ms": 10_000}}}
        }},
        "module_config": {"model": {"external": {
            "descriptor": ProcessModelDescriptor {
                adapter_id: "independent-model".into(),
                capabilities: ModelCapabilities::basic_text_and_tools(),
                hosted_tools: vec![],
            },
            "catalog_wait": true, "catalog_marker": marker, "catalog_release": release,
            "catalog": {"models": [{
                "id": "selected", "display_name": "Discovered model", "description": null,
                "hidden": false, "reasoning_efforts": ["high", "future"],
                "default_reasoning_effort": "future"
            }]}
        }}}
    }))
    .unwrap();
    let server = AgentAppServer::launch(
        config,
        config_dir.path().to_owned(),
        Some(&config_dir.path().join("config.toml")),
    )
    .await
    .unwrap();
    server.set_permission_mode(PermissionMode::Plan).await;
    let (shutdown, _) = broadcast::channel(1);
    let state = HttpAppState::new(server.clone(), shutdown, test_security()).await;
    let local_uri = session_uri("/config/local", &server);

    let local = read_local_config(&state, &local_uri).await;
    assert_eq!(local.model.as_ref().unwrap().name, "selected");
    assert_eq!(local.model_options[0].name, "selected");
    assert_eq!(local.permission_mode, "Plan");
    assert_eq!(local.reasoning.effort.as_deref(), Some("high"));
    assert!(local.model_catalog_error.is_none());
    assert!(
        !marker.exists(),
        "local configuration never starts discovery"
    );

    let full_state = state.clone();
    let full_uri = session_uri("/config", &server);
    let full =
        tokio::spawn(async move { route_request(full_state, authed_get_request(&full_uri)).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !marker.exists() {
            assert!(!full.is_finished(), "full config must wait for discovery");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("external catalog invocation starts");

    // Discovery must not hold the local configuration or settings behind its lock.
    let during_discovery = read_local_config(&state, &local_uri).await;
    assert_eq!(during_discovery, local);
    assert!(!full.is_finished(), "catalog has not been released");
    std::fs::write(&release, "ready").unwrap();
    let response = tokio::time::timeout(Duration::from_secs(3), full)
        .await
        .expect("catalog completes after release")
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let discovered: ConfigSummary =
        serde_json::from_slice(&response_bytes(response).await).unwrap();
    assert_eq!(discovered.model, local.model);
    assert_eq!(discovered.permission_mode, local.permission_mode);
    assert_eq!(discovered.model_options[0].label, "Discovered model");
    assert_eq!(discovered.reasoning.effort_options, ["high", "future"]);
    assert!(discovered.model_catalog_error.is_none());
    server.shutdown().await;
}

async fn read_local_config(state: &HttpAppState, uri: &str) -> ConfigSummary {
    let response = tokio::time::timeout(
        Duration::from_secs(1),
        route_request(state.clone(), authed_get_request(uri)),
    )
    .await
    .expect("local config must return without external discovery")
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&response_bytes(response).await).unwrap()
}
