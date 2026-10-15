//! User-supplied git catalogs. Distribution is a client concern; installing
//! an entry never adds a new agent runtime extension path.

pub mod git;
pub mod install;
pub mod open;
pub mod schema;
pub mod skills;

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use schema::Kind;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub url: String,
    #[serde(default)]
    pub branch: Option<String>,
}

impl Source {
    /// Stable across client builds; no dependence on Rust's hash algorithm.
    pub fn key(&self) -> String {
        key(&format!(
            "{}\n{}",
            self.url,
            self.branch.as_deref().unwrap_or_default()
        ))
    }

    /// `owner/repo` of the URL; titles a repository without an index.
    pub fn name(&self) -> String {
        let url = self.url.trim_end_matches('/');
        let url = url.strip_suffix(".git").unwrap_or(url);
        let mut parts = url
            .rsplit(['/', ':'])
            .filter(|part| !part.is_empty())
            .take(2)
            .collect::<Vec<_>>();
        parts.reverse();
        parts.join("/")
    }
}

pub fn key(text: &str) -> String {
    let hash = text.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub sources: Vec<Source>,
    pub installed: Vec<Installed>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Installed {
    pub source: Source,
    pub entry: String,
    pub version: String,
    pub kind: Kind,
    /// Agent Plugins and MCP receipts are scoped to the profile, not a conversation.
    pub profile: Option<PathBuf>,
    pub target: Target,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Folder {
        path: PathBuf,
    },
    Mcp {
        server: proteus_contracts::domain::ConfiguredMcpServerConfig,
    },
}

impl Preferences {
    pub fn remember(&mut self, installed: Installed) {
        self.installed.retain(|old| {
            old.source != installed.source
                || old.entry != installed.entry
                || old.profile != installed.profile
        });
        self.installed.push(installed);
    }
}

/// All filesystem locations are explicit in the installer, including tests.
#[derive(Clone)]
pub struct Locations {
    pub cache: PathBuf,
    pub extensions: PathBuf,
    pub plugins: PathBuf,
    pub skills: PathBuf,
}

impl Locations {
    pub fn local() -> Result<Self> {
        let home = PathBuf::from(std::env::var_os("HOME").context("Не задан HOME")?);
        let config = crate::preferences::config_dir().context("Не найдена папка настроек")?;
        let agent_home = std::env::var_os("PROTEUS_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".proteus"));
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(".cache"));
        Ok(Self {
            cache: cache.join(crate::APP_ID).join("catalogs"),
            extensions: config.join("extensions"),
            plugins: agent_home.join("plugins"),
            skills: agent_home.join("skills"),
        })
    }
}

#[cfg(test)]
mod tests;
