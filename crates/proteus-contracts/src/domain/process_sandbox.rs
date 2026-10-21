//! Host-owned policy for launching a child process. Concrete sandbox setup
//! belongs to the runtime; children cannot choose this policy through RPC.
use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessSandboxConfig {
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

impl Default for ProcessSandboxConfig {
    fn default() -> Self {
        Self::Trusted {}
    }
}

impl ProcessSandboxConfig {
    pub fn is_trusted(&self) -> bool {
        matches!(self, Self::Trusted {})
    }

    pub fn validate(&self, path: &str) -> Result<()> {
        if let Self::Restricted {
            read_only,
            read_write,
            ..
        } = self
            && read_only
                .iter()
                .chain(read_write)
                .any(|path| path.as_os_str().is_empty())
        {
            bail!("{path}: sandbox grant paths must not be empty");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ConfiguredMcpServerConfig;
    use serde_json::json;

    #[test]
    fn mcp_policy_is_strict_defaults_to_trusted_and_roundtrips_restrictions() {
        let mut value = json!({"name":"fixture", "command":"worker"});
        let trusted: ConfiguredMcpServerConfig = serde_json::from_value(value.clone()).unwrap();
        assert!(trusted.sandbox.is_trusted());
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
            assert!(serde_json::from_value::<ConfiguredMcpServerConfig>(value.clone()).is_err());
        }
        value["sandbox"] = json!({
            "mode":"restricted", "read_only":["."], "read_executable":true,
        });
        let server: ConfiguredMcpServerConfig = serde_json::from_value(value).unwrap();
        server.sandbox.validate("sandbox").unwrap();
        let serialized = serde_json::to_value(&server).unwrap();
        assert_eq!(serialized["sandbox"]["network"], false);
        assert_eq!(serialized["sandbox"]["read_executable"], true);
        assert_eq!(
            serde_json::from_value::<ConfiguredMcpServerConfig>(serialized).unwrap(),
            server
        );
        let invalid: ProcessSandboxConfig = serde_json::from_value(json!({
            "mode":"restricted", "read_only":[""],
        }))
        .unwrap();
        assert!(
            invalid
                .validate("sandbox")
                .unwrap_err()
                .to_string()
                .contains("must not be empty")
        );
    }
}
