use super::*;

struct RestrictedFixture {
    fixture: Fixture,
    workspace: PathBuf,
    state: PathBuf,
    secret: PathBuf,
}

impl RestrictedFixture {
    async fn registry(&self) -> proteus_contracts::contracts::ToolRegistry {
        let registry = self.fixture.registry().unwrap();
        if registry.get("fixture__sandbox_probe").is_none() {
            let mut config = self.fixture.config.clone();
            config.active_provider = None;
            config.providers.clear();
            let server = proteus_core::app_server::AgentAppServer::launch(
                config,
                self.fixture.dir.path().to_owned(),
                None,
            )
            .await
            .unwrap();
            let states = server.addons_snapshot().await.mcp_servers;
            server.shutdown().await;
            panic!(
                "restricted discovery failed: {states:?}; events={:?}",
                self.fixture.event_lines()
            );
        }
        registry
    }

    fn new(timeout_ms: u64, network: bool) -> Self {
        let mut fixture = Fixture::new(timeout_ms, None);
        let workspace = fixture.dir.path().join("workspace");
        let state = fixture.dir.path().join("state");
        let private = fixture.dir.path().join("private");
        for path in [&workspace, &state, &private] {
            std::fs::create_dir(path).unwrap();
        }
        let secret = private.join("secret.txt");
        std::fs::write(&secret, "synthetic private data\n").unwrap();
        std::fs::write(workspace.join("allowed.txt"), "synthetic workspace\n").unwrap();
        std::os::unix::fs::symlink(&secret, workspace.join("secret-link")).unwrap();
        fixture.events = state.join("events.log");
        fixture.generations = state.join("generations.log");
        let server = &mut fixture.config.tools.mcp_servers[0];
        server.cwd = Some("workspace".into());
        server.command = "python3".into();
        server.args.insert(0, "-B".into());
        server.environment.env.extend(BTreeMap::from([
            ("MCP_FIXTURE_SANDBOX".into(), "1".into()),
            (
                "MCP_FIXTURE_EVENT_LOG".into(),
                fixture.events.display().to_string(),
            ),
            (
                "MCP_FIXTURE_GENERATION_FILE".into(),
                fixture.generations.display().to_string(),
            ),
        ]));
        server.sandbox = serde_json::from_value(json!({
            "mode":"restricted", "read_executable":true,
            "read_only":["/usr", "/lib", "/lib64", "/bin", "workspace", server_path()],
            "read_write":["state"], "network":network,
        }))
        .unwrap();
        Self {
            fixture,
            workspace,
            state,
            secret,
        }
    }
}

#[tokio::test]
async fn restricted_mcp_discovers_multiple_tools_and_enforces_explicit_os_grants() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    for network in [false, true] {
        let restricted = RestrictedFixture::new(3_000, network);
        let fixture = &restricted.fixture;
        let registry = restricted.registry().await;
        assert!(
            registry.specs().len() >= 6,
            "MCP is not a single-export component"
        );
        let result = fixture
            .tool(&registry, "sandbox_probe")
            .invoke(
                &call(
                    "sandbox_probe",
                    json!({
                        "allowed_write":restricted.state.join("allowed.txt"),
                        "private_read":restricted.secret,
                        "private_write":restricted.secret.parent().unwrap().join("denied.txt"),
                        "port":listener.local_addr().unwrap().port(),
                    }),
                ),
                context(CancellationToken::new(), fixture.dir.path()),
            )
            .await
            .unwrap();
        assert!(result.ok, "{result:?}");
        let checks = &result.metadata["structured_content"];
        for key in [
            "workspace_read",
            "workspace_readonly",
            "private_read_denied",
            "symlink_read_denied",
            "private_write_denied",
            "home_absent",
        ] {
            assert_eq!(checks[key], true, "{key}: {checks}");
        }
        assert_eq!(checks["network_connected"], network);
        assert_eq!(checks["literal_env"], "literal-value");
        assert_eq!(
            checks["cwd"],
            restricted.workspace.to_string_lossy().as_ref()
        );
        assert_eq!(
            std::fs::read_to_string(restricted.state.join("allowed.txt")).unwrap(),
            "sandbox effect\n"
        );
        assert_eq!(
            std::fs::read_to_string(&restricted.secret).unwrap(),
            "synthetic private data\n"
        );
        assert!(!restricted.workspace.join("denied-write.txt").exists());
    }
}

#[tokio::test]
async fn restricted_mcp_cancel_and_timeout_stop_setsid_descendants_and_restart_explicitly() {
    for cancel_call in [true, false] {
        let restricted = RestrictedFixture::new(if cancel_call { 3_000 } else { 1_000 }, false);
        let fixture = &restricted.fixture;
        let registry = restricted.registry().await;
        let ready = restricted.state.join("child-ready");
        let heartbeat = restricted.state.join("heartbeat");
        let marker = restricted.state.join("late-effect");
        let cancellation = CancellationToken::new();
        let cancel = cancellation.clone();
        let tool = fixture.tool(&registry, "spawn_descendant");
        let tool_call = call(
            "spawn_descendant",
            json!({"ready":ready,"heartbeat":heartbeat,"marker":marker}),
        );
        let invoke = tool.invoke(&tool_call, context(cancellation, fixture.dir.path()));
        let wait_for_child = async {
            tokio::time::timeout(Duration::from_secs(2), async {
                while !ready.exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("child readiness before cancellation/timeout");
            if cancel_call {
                cancel.cancel();
            }
        };
        let (result, ()) = tokio::join!(invoke, wait_for_child);
        let error = result.expect_err("unresponsive server must be invalidated");
        assert!(
            error
                .to_string()
                .contains(if cancel_call { "cancel" } else { "1000ms" }),
            "{error:#}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        let settled = std::fs::read(&heartbeat).unwrap();
        tokio::time::sleep(Duration::from_millis(2_100)).await;
        assert_eq!(
            std::fs::read(&heartbeat).unwrap(),
            settled,
            "descendant still running"
        );
        assert!(
            !marker.exists(),
            "late side effect after generation teardown"
        );
        let result = fixture
            .tool(&registry, "echo")
            .invoke(
                &call("echo", json!({"label":"after teardown"})),
                context(CancellationToken::new(), fixture.dir.path()),
            )
            .await
            .unwrap();
        assert_echo_output(&result, "after teardown", 2);
    }
}

#[tokio::test]
async fn invalid_mcp_grants_or_missing_runtime_never_fall_back_to_trusted_execution() {
    for missing_runtime in [false, true] {
        let mut restricted = RestrictedFixture::new(1_000, false);
        restricted.fixture.config.tools.mcp_servers[0].sandbox = serde_json::from_value(json!({
            "mode":"restricted", "read_executable":true,
            "read_only":if missing_runtime { json!(["workspace", server_path()]) } else { json!(["absent-grant"]) },
            "read_write":["state"],
        })).unwrap();
        let mut config = restricted.fixture.config.clone();
        config.active_provider = None;
        config.providers.clear();
        let server = proteus_core::app_server::AgentAppServer::launch(
            config,
            restricted.fixture.dir.path().to_owned(),
            None,
        )
        .await
        .unwrap();
        let states = server.addons_snapshot().await.mcp_servers;
        assert!(states[0].tools.is_empty());
        assert!(states[0].error.is_some(), "{states:?}");
        assert!(
            !restricted.fixture.generations.exists(),
            "server executed without its restrictions"
        );
        server.shutdown().await;
    }
}
