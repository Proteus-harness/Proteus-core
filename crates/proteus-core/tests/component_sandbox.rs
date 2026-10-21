#![cfg(target_os = "linux")]

#[path = "support/model.rs"]
mod test_model;

use std::{fs, path::Path};

use anyhow::Result;
use proteus_contracts::{
    contracts::{ExecutionAttribution, ToolContext},
    domain::{ToolCall, new_call_id, new_execution_id},
};
use proteus_core::{core::RuntimeRegistry, process_adapters::ProcessComponentConfig};
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restricted_config_enforces_os_access_through_the_registered_tool() -> Result<()> {
    let state = tempfile::tempdir()?;
    let workspace = state.path().join("workspace");
    fs::create_dir(&workspace)?;
    let secret = state.path().join("private.txt");
    fs::write(&secret, "synthetic secret\n")?;
    std::os::unix::fs::symlink(&secret, workspace.join("secret-link"))?;
    // The sandbox has its own writable /tmp. Mount setup creates private
    // ancestors of workspace there, so writing an adjacent basename can
    // legitimately succeed without touching the host. Probe an existing host
    // directory which is absent from that private filesystem instead.
    let private = state.path().join("private");
    fs::create_dir(&private)?;
    let outside = private.join("outside-write.txt");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let worker = root.join("crates/proteus-core/tests/fixtures/component_sandbox.py");
    let helpers = root.join("examples/modules");
    let component: ProcessComponentConfig = serde_json::from_value(json!({
        "command":"/usr/bin/python3", "args":["-B", worker],
        "env":{"SANDBOX_VISIBLE":"yes"},
        "sandbox":{
            "mode":"restricted",
            "read_only":["/usr", "/lib64", worker, helpers],
            "read_write":["."],
        },
        "handshake_timeout_ms":3000,
        "exports":{"tool":{"sandbox.fixture":{"timeout_ms":3000}}},
    }))?;
    let mut config = test_model::config();
    config.tools.enabled = vec!["sandbox_probe".into()];
    config
        .components
        .insert("sandbox-fixture".into(), component);
    let registry = RuntimeRegistry::from_config(&config, workspace.clone())?;
    let result = registry
        .tools
        .get("sandbox_probe")
        .expect("registered tool")
        .invoke(
            &ToolCall::new(
                new_call_id(),
                "sandbox_probe",
                json!({"secret":secret, "outside":outside}),
            ),
            ToolContext::new(
                workspace.clone(),
                ExecutionAttribution::detached(new_execution_id()),
            ),
        )
        .await?;
    assert!(result.ok, "{}", result.output);
    let checks: Value = serde_json::from_str(&result.output)?;
    for label in [
        "workspace_read_write",
        "explicit_environment",
        "secret_denied",
        "symlink_denied",
        "outside_write_denied",
    ] {
        assert_eq!(checks[label], true, "{label}: {checks}");
    }
    assert_eq!(
        fs::read_to_string(workspace.join("allowed.txt"))?,
        "workspace effect\n"
    );
    assert_eq!(fs::read_to_string(secret)?, "synthetic secret\n");
    assert!(!outside.exists());
    Ok(())
}
