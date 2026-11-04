//! Validation and public profile projection for top-level `agent_control`.

use anyhow::{Context, Result, bail};

use crate::{
    contracts::{AgentIsolation, AgentProfile},
    core::AgentProfileConfig,
};

pub(super) type ProcessRoleConfig = AgentProfileConfig;

/// Set only by the parent host when it creates a process peer. Request/tool
/// metadata cannot replenish an inherited execution budget.
pub(super) const DEPTH_BUDGET_ENV: &str = "PROTEUS_AGENT_CONTROL_REMAINING_DEPTH";

pub(super) fn effective_depth_budget(configured: u64) -> Result<u64> {
    match std::env::var(DEPTH_BUDGET_ENV) {
        Ok(inherited) => inherited
            .parse::<u64>()
            .map(|remaining| configured.min(remaining))
            .with_context(|| {
                format!("invalid inherited agent-control depth budget: {DEPTH_BUDGET_ENV}")
            }),
        Err(std::env::VarError::NotPresent) => Ok(configured),
        Err(error) => Err(error).context("invalid inherited agent-control depth budget"),
    }
}

fn parse_isolation(value: Option<&str>) -> Result<AgentIsolation> {
    match value {
        None => Ok(AgentIsolation::None),
        Some("worktree") => Ok(AgentIsolation::Worktree),
        Some(other) => bail!("unknown isolation value: {other} (expected \"worktree\")"),
    }
}

pub(super) fn build_agent_profiles(roles: &[ProcessRoleConfig]) -> Result<Vec<AgentProfile>> {
    let mut profiles: Vec<AgentProfile> = Vec::with_capacity(roles.len());
    for role in roles {
        if role.name.trim().is_empty() {
            bail!("agent profile name must not be empty");
        }
        if role.config.trim().is_empty() {
            bail!("agent profile {} must set a child config", role.name);
        }
        if profiles.iter().any(|existing| existing.name == role.name) {
            bail!("duplicate agent profile: {}", role.name);
        }
        let isolation = parse_isolation(role.isolation.as_deref())
            .with_context(|| format!("agent profile {}", role.name))?;
        profiles.push(
            AgentProfile::new(role.name.clone(), role.description.clone())
                .with_parallel_safe(role.parallel_safe)
                .with_isolation(isolation),
        );
    }
    Ok(profiles)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(name: &str, config: &str) -> ProcessRoleConfig {
        ProcessRoleConfig {
            name: name.to_owned(),
            description: "Explorer".to_owned(),
            config: config.to_owned(),
            args: Vec::new(),
            parallel_safe: false,
            isolation: None,
            max_processes: None,
            timeout_ms: None,
            max_summary_bytes: None,
        }
    }

    #[test]
    fn projects_only_control_plane_profile_fields() {
        let mut configured = role("explore", "codex-explorer");
        configured.parallel_safe = true;
        configured.timeout_ms = Some(60_000);
        configured.max_summary_bytes = Some(2_048);

        let profiles = build_agent_profiles(&[configured]).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "explore");
        assert!(profiles[0].parallel_safe);
    }

    #[test]
    fn missing_child_config_and_duplicates_are_rejected() {
        assert!(build_agent_profiles(&[role("explore", "  ")]).is_err());
        let duplicate = role("explore", "codex-explorer");
        assert!(build_agent_profiles(&[duplicate.clone(), duplicate]).is_err());
    }
}
