use super::*;

fn fixture_with_list_mode(mode: &str) -> Fixture {
    let mut fixture = Fixture::new(1_000, None);
    fixture.config.tools.mcp_servers[0]
        .environment
        .env
        .insert("MCP_FIXTURE_LIST_MODE".into(), mode.into());
    fixture
}

#[tokio::test]
async fn input_schema_discovery_errors_are_isolated_without_registering_a_prefix() {
    for (mode, enabled) in [
        ("invalid_first", true),
        ("invalid_last", true),
        ("empty", true),
        ("invalid_last", false),
    ] {
        let fixture = fixture_with_list_mode(mode);
        let healthy = Fixture::new(1_000, None);
        let mut config = fixture.config.clone();
        config.active_provider = None;
        config.providers.clear();
        config.tools.mcp_servers[0].enabled = enabled;
        let mut healthy_config = healthy.config.tools.mcp_servers[0].clone();
        healthy_config.name = "healthy".into();
        config.tools.mcp_servers.push(healthy_config);
        let server = proteus_core::app_server::AgentAppServer::launch(
            config,
            fixture.dir.path().to_owned(),
            None,
        )
        .await
        .expect("one invalid discovery must not prevent independent server assembly");
        let snapshot = server.addons_snapshot().await;
        let state = snapshot
            .mcp_servers
            .iter()
            .find(|state| state.name == "fixture")
            .unwrap();
        assert_eq!(state.enabled, enabled);
        assert!(state.tools.is_empty(), "no prefix from {mode}");
        if enabled && mode != "empty" {
            let error = state.error.as_ref().expect("schema error must be visible");
            assert!(error.contains("fixture__invalid"), "{error}");
            assert!(error.contains("invalid input schema"), "{error}");
        } else {
            assert!(state.error.is_none());
        }
        let healthy_state = snapshot
            .mcp_servers
            .iter()
            .find(|state| state.name == "healthy")
            .unwrap();
        assert!(healthy_state.error.is_none());
        assert!(healthy_state.tools.contains(&"healthy__echo".to_owned()));
        let tools = server.config_builder_snapshot().await.tools;
        assert!(!tools.iter().any(|tool| tool.name.starts_with("fixture__")));
        assert!(
            tools
                .iter()
                .any(|tool| tool.name == "healthy__echo" && tool.registered)
        );
        server.shutdown().await;
    }
}

#[test]
fn valid_discovered_batches_keep_name_collisions_fatal() {
    let fixture = fixture_with_list_mode("collision");
    let error = fixture.registry().err().expect("duplicate discovered name");
    assert!(format!("{error:#}").contains("duplicate tool registration"));

    let mut first = Fixture::new(1_000, None);
    let second = Fixture::new(1_000, None);
    first.config.tools.mcp_servers[0].name = "same name".into();
    let mut second_config = second.config.tools.mcp_servers[0].clone();
    second_config.name = "same.name".into();
    first.config.tools.mcp_servers.push(second_config);
    let error = first
        .registry()
        .err()
        .expect("sanitized cross-server name collision");
    assert!(format!("{error:#}").contains("duplicate tool registration"));
}

#[tokio::test]
async fn invalid_schema_batch_is_rejected_before_potential_name_collisions() {
    let fixture = fixture_with_list_mode("invalid_collision");
    let mut config = fixture.config.clone();
    config.active_provider = None;
    config.providers.clear();
    let server = proteus_core::app_server::AgentAppServer::launch(
        config,
        fixture.dir.path().to_owned(),
        None,
    )
    .await
    .expect("invalid batch is a server-local discovery error");
    let snapshot = server.addons_snapshot().await;
    let state = snapshot
        .mcp_servers
        .iter()
        .find(|state| state.name == "fixture")
        .unwrap();
    assert!(state.tools.is_empty());
    let error = state.error.as_ref().expect("invalid input schema");
    assert!(error.contains("invalid input schema"), "{error}");
    assert!(!error.contains("duplicate tool registration"), "{error}");
    assert!(
        !server
            .config_builder_snapshot()
            .await
            .tools
            .iter()
            .any(|tool| tool.name.starts_with("fixture__"))
    );
    server.shutdown().await;
}
