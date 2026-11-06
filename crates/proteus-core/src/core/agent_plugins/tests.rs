use super::*;

#[test]
fn portable_manifest_and_mcp_keep_independent_failure_boundaries() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().join("plugin");
    std::fs::create_dir_all(root.join("skills")).unwrap();
    std::fs::write(root.join("plugin.json"),serde_json::json!({"$schema":manifest::PLUGIN_SCHEMA,"name":"test.plugin","unknown":"ignored","extensions":{"org.other":17}}).to_string()).unwrap();
    std::fs::write(root.join("mcp.json"),serde_json::json!({"$schema":mcp::MCP_SCHEMA,"mcpServers":{
        "valid":{"type":"stdio","command":"python3","args":["${PLUGIN_ROOT}/server.py"],"env":{"DATA":"${PLUGIN_DATA}"}},
        "bad":{"type":"stdio","command":"../escape"},
        "remote":{"type":"streamable-http","url":"https://example.com/mcp"}
    }}).to_string()).unwrap();
    let config = AddonConfig {
        disabled_mcp_servers: vec![],
        disabled_skills: vec![],
        plugins: vec![crate::domain::AgentPluginConfig {
            path: root.clone(),
            enabled: true,
            data_key: None,
        }],
    };
    let result = resolve(&config, workspace.path());
    assert!(result.plugins[0].error.is_none());
    assert_eq!(result.skills.packages.len(), 1);
    assert_eq!(result.servers.len(), 1);
    assert_eq!(result.servers[0].name, "test.plugin:valid");
    assert_eq!(result.servers[0].cwd.as_deref(), Some(root.as_path()));
    assert!(
        result.plugins[0]
            .warnings
            .iter()
            .any(|warning| warning.contains("invalid MCP server"))
    );
    assert!(
        result.plugins[0]
            .warnings
            .iter()
            .any(|warning| warning.contains("unsupported MCP transport"))
    );
}

#[test]
fn state_key_survives_version_replacement_and_rollback_without_copying_data() {
    let workspace = tempfile::tempdir().unwrap();
    let roots = ["v1", "v2"].map(|version| {
        let root = workspace.path().join(version);
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("plugin.json"),
            serde_json::json!({
                "$schema": manifest::PLUGIN_SCHEMA, "name": "stateful"
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            root.join("mcp.json"),
            serde_json::json!({
                "$schema": mcp::MCP_SCHEMA, "mcpServers": {
                    "server": {"type": "stdio", "command": "python3",
                               "env": {"DATA": "${PLUGIN_DATA}"}}
                }
            })
            .to_string(),
        )
        .unwrap();
        root.canonicalize().unwrap()
    });
    let mut config = AddonConfig {
        plugins: vec![crate::domain::AgentPluginConfig {
            path: roots[0].clone(),
            enabled: true,
            data_key: None,
        }],
        ..Default::default()
    };
    let address = |config: &AddonConfig, cwd: &Path| {
        let resolved = resolve(config, cwd);
        assert!(resolved.plugins[0].error.is_none());
        assert_eq!(resolved.servers.len(), 1);
        std::path::PathBuf::from(&resolved.servers[0].environment.env["DATA"])
    };
    let initial = address(&config, workspace.path());
    std::fs::write(initial.join("state.txt"), "retained state").unwrap();

    // Pin the existing default identity before replacing the version path.
    config.plugins[0].data_key = Some(roots[0].to_string_lossy().into_owned());
    config.plugins[0].path = roots[1].clone();
    assert_eq!(address(&config, workspace.path()), initial);
    assert_eq!(
        std::fs::read_to_string(initial.join("state.txt")).unwrap(),
        "retained state"
    );
    config.plugins[0].path = roots[0].clone();
    config.plugins[0].enabled = false;
    assert_eq!(address(&config, workspace.path()), initial);
    assert!(!resolve(&config, workspace.path()).servers[0].enabled);

    // A key identifies this installation, while its data remain workspace-local.
    let other_workspace = tempfile::tempdir().unwrap();
    config.plugins[0].enabled = true;
    let other = address(&config, other_workspace.path());
    assert_ne!(other, initial);
    assert!(!other.join("state.txt").exists());
    config.plugins[0].data_key = Some(" ".into());
    let invalid = resolve(&config, workspace.path());
    assert!(invalid.servers.is_empty());
    assert!(
        invalid.plugins[0]
            .error
            .as_deref()
            .unwrap()
            .contains("data_key")
    );
}

#[test]
fn placeholder_expansion_is_non_recursive() {
    assert_eq!(
        paths::expand(
            "${PLUGIN_ROOT}/${OTHER}/${PLUGIN_DATA}",
            Path::new("/root/${PLUGIN_DATA}"),
            Path::new("/data")
        ),
        "/root/${PLUGIN_DATA}/${OTHER}//data"
    );
}
