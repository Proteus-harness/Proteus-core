use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use proteus_process_host::ProcessSandbox;
use serde::{Deserialize, Serialize};

use crate::core::expand_user_path;

/// Host-owned launch policy. This is deliberately outside the component's
/// opaque module_config and initialize payload: the child cannot choose it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ComponentSandboxConfig {
    Trusted {},
    Restricted {
        #[serde(default)]
        read_only: Vec<PathBuf>,
        #[serde(default)]
        read_write: Vec<PathBuf>,
        #[serde(default)]
        network: bool,
        #[serde(default)]
        read_executable: bool,
    },
}

impl Default for ComponentSandboxConfig {
    fn default() -> Self {
        Self::Trusted {}
    }
}

impl ComponentSandboxConfig {
    pub(super) fn is_trusted(&self) -> bool {
        matches!(self, Self::Trusted {})
    }

    pub(super) fn validate(&self, exports: usize, path: &str) -> Result<()> {
        if let Self::Restricted {
            read_only,
            read_write,
            ..
        } = self
        {
            // The first mode deliberately avoids giving a multi-export process
            // a union of OS privileges and mistaking callback authority for
            // isolation between its exports.
            if exports != 1 {
                bail!(
                    "{path}: restricted components require exactly one export; split exports into separate components"
                );
            }
            if read_only
                .iter()
                .chain(read_write)
                .any(|p| p.as_os_str().is_empty())
            {
                bail!("{path}: sandbox grant paths must not be empty");
            }
        }
        Ok(())
    }

    pub(super) fn resolve(&self, workspace: &Path) -> Result<Option<ProcessSandbox>> {
        let Self::Restricted {
            read_only,
            read_write,
            network,
            read_executable,
        } = self
        else {
            return Ok(None);
        };
        // Preserve logical destinations such as /lib64. The host separately
        // canonicalizes and pins mount sources; erasing aliases here would
        // remove the executable's dynamic-loader path inside the empty root.
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
        let config: ComponentSandboxConfig = serde_json::from_value(json!({
            "mode":"restricted", "read_only":["worker.py", "/lib64"], "read_write":["."],
            "read_executable":true,
        }))
        .unwrap();
        let sandbox = config.resolve(Path::new("/workspace")).unwrap().unwrap();
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
}
