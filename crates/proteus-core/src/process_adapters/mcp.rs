//! Host-owned launch setup shared by MCP discovery and CLI inspection.
use std::path::Path;

use anyhow::{Context, Result};
use proteus_process_host::ProcessSpec;

use crate::domain::ConfiguredMcpServerConfig;

/// Resolves the same host-owned launch policy used for MCP discovery and every
/// subsequent server generation. Restricted launch failures never retry trusted.
pub fn mcp_server_process_spec(
    server: &ConfiguredMcpServerConfig,
    workspace: &Path,
) -> Result<ProcessSpec> {
    server.sandbox.validate("MCP server sandbox")?;
    let workspace = std::fs::canonicalize(workspace)
        .with_context(|| format!("failed to resolve MCP workspace {}", workspace.display()))?;
    let mut spec = ProcessSpec::new(server.command.clone())
        .args(server.args.clone())
        .env_allowlist(server.environment.env_allowlist.clone())
        .envs(server.environment.env.clone())
        .cwd(super::sandbox::resolve_process_cwd(
            server.cwd.as_deref(),
            &workspace,
        )?);
    if let Some(sandbox) = super::sandbox::resolve_sandbox(&server.sandbox, &workspace)? {
        spec = spec.sandbox(sandbox);
    }
    spec.validate_launch()?;
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mcp_cwd_resolves_once_against_a_canonical_relative_workspace() {
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        std::fs::create_dir(workspace.path().join("child")).unwrap();
        let relative = Path::new(workspace.path().file_name().unwrap());
        for cwd in [None, Some("child")] {
            let server: ConfiguredMcpServerConfig = serde_json::from_value(json!({
                "name":"fixture", "command":"worker", "cwd":cwd,
                "env":{"PROTEUS_MCP_FIXTURE":"child"},
            }))
            .unwrap();
            let spec = mcp_server_process_spec(&server, relative).unwrap();
            let canonical = workspace.path().canonicalize().unwrap();
            assert_eq!(
                spec.cwd,
                Some(cwd.map_or_else(|| canonical.clone(), |cwd| canonical.join(cwd)))
            );
            assert!(spec.sandbox.is_none());
            assert_eq!(
                spec.resolved_environment().unwrap()["PROTEUS_MCP_FIXTURE"],
                "child"
            );
        }
    }
}
