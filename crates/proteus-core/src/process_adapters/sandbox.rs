use std::path::{Path, PathBuf};

use anyhow::Result;
use proteus_contracts::domain::ProcessSandboxConfig;
use proteus_process_host::ProcessSandbox;

use crate::core::expand_user_path;

/// Resolves grants against the canonical workspace while preserving logical
/// destinations such as /lib64. The process host pins their canonical sources.
pub(crate) fn resolve_sandbox(
    config: &ProcessSandboxConfig,
    workspace: &Path,
) -> Result<Option<ProcessSandbox>> {
    config.validate("process.sandbox")?;
    let ProcessSandboxConfig::Restricted {
        read_only,
        read_write,
        network,
        read_executable,
    } = config
    else {
        return Ok(None);
    };
    let paths = |configured: &[PathBuf]| {
        configured
            .iter()
            .map(|path| {
                let path = expand_user_path(path);
                if path.is_absolute() {
                    path
                } else {
                    workspace.join(path)
                }
            })
            .collect()
    };
    Ok(Some(ProcessSandbox {
        read_only: paths(read_only),
        read_write: paths(read_write),
        network: *network,
        read_executable: *read_executable,
    }))
}

pub(crate) fn resolve_process_cwd(configured: Option<&Path>, workspace: &Path) -> Result<PathBuf> {
    let path = configured.map_or_else(|| workspace.to_path_buf(), expand_user_path);
    let path = if path.is_relative() {
        workspace.join(path)
    } else {
        path
    };
    // Workspace is canonicalized once by the caller. Keep explicit aliases:
    // read_executable=false grants retain their logical mount destinations.
    // The process host canonicalizes cwd only when executable pinning needs it.
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process_adapters::ProcessComponentConfig;
    use serde_json::json;

    #[test]
    fn launch_policy_defaults_to_trusted_and_rejects_unknown_or_ambiguous_forms() {
        let mut value = json!({"command":"worker", "exports":{"tool":{"fixture":{}}}});
        let trusted: ProcessComponentConfig = serde_json::from_value(value.clone()).unwrap();
        trusted
            .validate_for("fixture", "components.fixture")
            .unwrap();
        assert!(
            serde_json::to_value(trusted)
                .unwrap()
                .get("sandbox")
                .is_none()
        );
        for sandbox in [
            json!({"mode":"unknown"}),
            json!({"mode":"trusted", "network":true}),
            json!({"mode":"trusted", "read_executable":true}),
            json!({"mode":"restricted", "unexpected":true}),
        ] {
            value["sandbox"] = sandbox;
            assert!(serde_json::from_value::<ProcessComponentConfig>(value.clone()).is_err());
        }
    }

    #[test]
    fn restricted_component_rejects_shared_exports_and_empty_grants() {
        for exports in [
            json!({"tool":{"one":{}, "two":{}}}),
            json!({"tool":{"one":{}}, "context_provider":{"one":{}}}),
        ] {
            let component: ProcessComponentConfig = serde_json::from_value(json!({
                "command":"worker", "sandbox":{"mode":"restricted"}, "exports":exports,
            }))
            .unwrap();
            assert!(
                component
                    .validate_for("fixture", "components.fixture")
                    .unwrap_err()
                    .to_string()
                    .contains("exactly one export")
            );
        }
        let component: ProcessComponentConfig = serde_json::from_value(json!({
            "command":"worker", "sandbox":{"mode":"restricted", "read_write":[""]},
            "exports":{"tool":{"one":{}}},
        }))
        .unwrap();
        assert!(
            component
                .validate_for("fixture", "components.fixture")
                .unwrap_err()
                .to_string()
                .contains("must not be empty")
        );
    }

    #[test]
    fn sandbox_paths_are_workspace_relative_without_implicit_grants_or_alias_loss() {
        let config: ProcessSandboxConfig = serde_json::from_value(json!({
            "mode":"restricted", "read_only":["worker.py", "/lib64"], "read_write":["."],
            "read_executable":true,
        }))
        .unwrap();
        let sandbox = resolve_sandbox(&config, Path::new("/workspace"))
            .unwrap()
            .unwrap();
        assert_eq!(
            sandbox.read_only,
            [
                PathBuf::from("/workspace/worker.py"),
                PathBuf::from("/lib64")
            ]
        );
        assert_eq!(sandbox.read_write, [PathBuf::from("/workspace/.")]);
        assert!(!sandbox.network);
        assert!(sandbox.read_executable);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn relative_workspace_resolves_cwd_and_grants_once_for_doctor_and_launch() {
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let relative = Path::new(workspace.path().file_name().unwrap());
        let canonical = workspace.path().canonicalize().unwrap();
        std::fs::create_dir(workspace.path().join("child")).unwrap();
        for restricted in [false, true] {
            for cwd in [None, Some("child")] {
                let mut value = json!({"command":"worker", "exports":{"tool":{"fixture":{}}}});
                if let Some(cwd) = cwd {
                    value["cwd"] = json!(cwd);
                }
                if restricted {
                    value["sandbox"] = json!({"mode":"restricted", "read_write":["."]});
                }
                let config: ProcessComponentConfig = serde_json::from_value(value).unwrap();
                let spec = config.process_spec(relative).unwrap();
                assert_eq!(
                    spec.cwd,
                    Some(cwd.map_or_else(|| canonical.clone(), |cwd| canonical.join(cwd)))
                );
                if let Some(sandbox) = spec.sandbox {
                    assert_eq!(sandbox.read_write, [canonical.join(".")]);
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn logical_alias_cwd_remains_reachable_without_executable_grant() {
        let workspace = tempfile::tempdir().unwrap();
        let actual = tempfile::tempdir().unwrap();
        let alias = workspace.path().join("alias");
        std::os::unix::fs::symlink(actual.path(), &alias).unwrap();
        let component: ProcessComponentConfig = serde_json::from_value(json!({
            "command":"worker", "cwd":"alias", "exports":{"tool":{"fixture":{}}},
            "sandbox":{"mode":"restricted", "read_only":["alias"]},
        }))
        .unwrap();
        let spec = component.process_spec(workspace.path()).unwrap();
        assert_eq!(spec.cwd, Some(alias.clone()));
        assert_eq!(spec.sandbox.unwrap().read_only, [alias.clone()]);
        let server: proteus_contracts::domain::ConfiguredMcpServerConfig =
            serde_json::from_value(json!({
                "name":"fixture", "command":"worker", "cwd":"alias",
                "sandbox":{"mode":"restricted", "read_only":["alias"]},
            }))
            .unwrap();
        let spec =
            crate::process_adapters::mcp_server_process_spec(&server, workspace.path()).unwrap();
        assert_eq!(spec.cwd, Some(alias));
    }
}
