//! Read-only analytics endpoints: usage, turn analysis and build topology.

use std::path::Path;

use anyhow::{Result, bail};
use proteus_contracts::app_protocol::analysis::AppSessionAnalysis;
use proteus_contracts::app_protocol::topology::TopologySnapshot;
use proteus_contracts::domain::SessionUsageSnapshot;

use super::Api;

impl Api {
    /// `None` while a live session has not initialized its usage journal.
    pub fn usage(&self, session_dir: &Path) -> Result<Option<SessionUsageSnapshot>> {
        self.get("/usage", Some(session_dir))
    }

    /// The latest turn when `turn` is `None`.
    pub fn analysis(&self, session_dir: &Path, turn: Option<&str>) -> Result<AppSessionAnalysis> {
        match turn {
            Some(turn) => self.get_with("/analysis", Some(session_dir), &[("turn_id", turn)]),
            None => self.get("/analysis", Some(session_dir)),
        }
    }

    /// Topology of a resumed session's build.
    pub fn topology(&self, session_dir: &Path) -> Result<TopologySnapshot> {
        self.get("/inspect/topology", Some(session_dir))
    }

    pub fn topology_mermaid(&self, session_dir: &Path) -> Result<String> {
        let response = self
            .agent
            .get(format!("{}/inspect/topology.mmd", self.origin))
            .header("Authorization", format!("Bearer {}", self.token))
            .query("session_dir", session_dir.to_string_lossy())
            .call()?;
        let status = response.status();
        let body = response.into_body().into_with_config().read_to_string()?;
        if !status.is_success() {
            bail!("GET /inspect/topology.mmd failed with {status}: {body}");
        }
        Ok(body)
    }
}
