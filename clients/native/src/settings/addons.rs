//! Skills, MCP servers and Agent Plugins packages of the profile: edits of
//! the draft and lookups in the server's addons snapshot.

use std::path::{Path, PathBuf};

use proteus_contracts::app_protocol::addons::{
    AppAddonsSnapshot, AppAddonsUpdate, AppAgentPluginState, AppMcpServerState,
};
use proteus_contracts::domain::{AgentPluginConfig, ConfiguredMcpServerConfig, SkillDescriptor};

use super::draft::Draft;

/// Disabled lists are sets: kept sorted so switching an entry back and forth
/// leaves the draft equal to its baseline.
pub fn normalized(mut addons: AppAddonsUpdate) -> AppAddonsUpdate {
    for list in [
        &mut addons.addons.disabled_skills,
        &mut addons.addons.disabled_mcp_servers,
    ] {
        list.sort();
        list.dedup();
    }
    addons
}

pub(super) fn set_listed(list: &mut Vec<String>, item: &str, listed: bool) {
    match (
        list.binary_search_by(|entry| entry.as_str().cmp(item)),
        listed,
    ) {
        (Err(ix), true) => list.insert(ix, item.to_string()),
        (Ok(ix), false) => {
            list.remove(ix);
        }
        _ => {}
    }
}

impl Draft {
    pub fn skill_enabled(&self, id: &str) -> bool {
        !self
            .addons
            .addons
            .disabled_skills
            .iter()
            .any(|skill| skill == id)
    }

    pub fn set_skill(&mut self, id: &str, enabled: bool) {
        set_listed(&mut self.addons.addons.disabled_skills, id, !enabled);
    }

    /// A server runs unless its profile entry or `disabled_mcp_servers`
    /// switches it off.
    pub fn mcp_enabled(&self, name: &str) -> bool {
        !self
            .addons
            .addons
            .disabled_mcp_servers
            .iter()
            .any(|server| server == name)
            && self
                .configured_mcp(name)
                .is_none_or(|server| server.enabled)
    }

    /// Switches a server of any origin through `disabled_mcp_servers`;
    /// switching on a profile server also clears its own `enabled = false`.
    pub fn set_mcp(&mut self, name: &str, enabled: bool) {
        set_listed(&mut self.addons.addons.disabled_mcp_servers, name, !enabled);
        if enabled
            && let Some(server) = self
                .addons
                .mcp_servers
                .iter_mut()
                .find(|server| server.name == name)
        {
            server.enabled = true;
        }
    }

    pub fn configured_mcp(&self, name: &str) -> Option<&ConfiguredMcpServerConfig> {
        self.addons
            .mcp_servers
            .iter()
            .find(|server| server.name == name)
    }

    pub fn set_package(&mut self, path: &Path, enabled: bool) {
        for plugin in &mut self.addons.addons.plugins {
            if plugin.path == path {
                plugin.enabled = enabled;
            }
        }
    }

    /// Lists a package folder, or switches on the entry that already
    /// lists it.
    pub fn add_package(&mut self, path: PathBuf) {
        let plugins = &mut self.addons.addons.plugins;
        match plugins.iter_mut().find(|plugin| plugin.path == path) {
            Some(plugin) => plugin.enabled = true,
            None => plugins.push(AgentPluginConfig {
                path,
                enabled: true,
            }),
        }
    }

    pub fn remove_package(&mut self, path: &Path) {
        self.addons
            .addons
            .plugins
            .retain(|plugin| plugin.path != path);
    }
}

/// MCP server known to the profile or to the last assembly.
pub struct McpServer<'a> {
    pub name: &'a str,
    /// Discovery result of the saved assembly.
    pub state: Option<&'a AppMcpServerState>,
    /// Position and entry in the draft's own `tools.mcp_servers`.
    pub configured: Option<(usize, &'a ConfiguredMcpServerConfig)>,
    /// Package that brings the server, by its `package:server` name.
    pub package: Option<&'a str>,
}

/// Profile servers of the draft in their order, then the package servers
/// of the last assembly in its order.
pub fn mcp_servers<'a>(
    draft: &'a Draft,
    addons: Option<&'a AppAddonsSnapshot>,
) -> Vec<McpServer<'a>> {
    let saved = |name: &str| {
        addons.and_then(|addons| {
            addons
                .settings
                .mcp_servers
                .iter()
                .any(|server| server.name == name)
                .then(|| addons.mcp_servers.iter().find(|state| state.name == name))
                .flatten()
        })
    };
    let mut servers: Vec<McpServer<'a>> = draft
        .addons
        .mcp_servers
        .iter()
        .enumerate()
        .map(|(ix, server)| McpServer {
            name: &server.name,
            state: saved(&server.name),
            configured: Some((ix, server)),
            package: None,
        })
        .collect();
    if let Some(addons) = addons {
        servers.extend(package_servers(addons).map(|state| McpServer {
            name: &state.name,
            state: Some(state),
            configured: None,
            package: package_of(addons, &state.name),
        }));
    }
    servers
}

/// Servers of the last assembly that the saved profile does not list.
pub fn package_servers(addons: &AppAddonsSnapshot) -> impl Iterator<Item = &AppMcpServerState> {
    addons.mcp_servers.iter().filter(|state| {
        !addons
            .settings
            .mcp_servers
            .iter()
            .any(|server| server.name == state.name)
    })
}

/// Name of the loaded package whose `package:item` prefix `name` carries.
pub fn package_of<'a>(addons: &'a AppAddonsSnapshot, name: &str) -> Option<&'a str> {
    let (prefix, _) = name.split_once(':')?;
    addons
        .plugins
        .iter()
        .filter_map(|plugin| plugin.name.as_deref())
        .find(|package| *package == prefix)
}

/// MCP server that registered a tool in the last assembly.
pub fn tool_server<'a>(addons: &'a AppAddonsSnapshot, tool: &str) -> Option<&'a str> {
    addons
        .mcp_servers
        .iter()
        .find(|server| server.tools.iter().any(|name| name == tool))
        .map(|server| server.name.as_str())
}

/// Every skill of the providers' catalogs with the provider that found it.
pub fn skills(addons: &AppAddonsSnapshot) -> Vec<(&str, &SkillDescriptor)> {
    addons
        .catalogs
        .iter()
        .filter_map(|catalog| Some((catalog.provider.as_str(), catalog.catalog.as_ref()?)))
        .flat_map(|(provider, catalog)| catalog.skills.iter().map(move |skill| (provider, skill)))
        .collect()
}

/// Disabled skill ids that no catalog lists. Unknown while a provider
/// failed, since its skills are not visible.
pub fn missing_skills<'a>(draft: &'a Draft, addons: &AppAddonsSnapshot) -> Vec<&'a str> {
    if addons
        .catalogs
        .iter()
        .any(|catalog| catalog.error.is_some())
    {
        return Vec::new();
    }
    let known = skills(addons);
    draft
        .addons
        .addons
        .disabled_skills
        .iter()
        .filter(|id| !known.iter().any(|(_, skill)| &skill.id == *id))
        .map(String::as_str)
        .collect()
}

/// Load state of a package of the saved profile, matched by its entry.
pub fn package_state<'a>(
    addons: &'a AppAddonsSnapshot,
    path: &Path,
) -> Option<&'a AppAgentPluginState> {
    let ix = addons
        .settings
        .addons
        .plugins
        .iter()
        .position(|plugin| plugin.path == path)?;
    addons.plugins.get(ix)
}

/// Skills and MCP servers a loaded package contributes.
pub fn package_contents(addons: &AppAddonsSnapshot, package: &str) -> (usize, usize) {
    let owned = |name: &str| {
        name.split_once(':')
            .is_some_and(|(prefix, _)| prefix == package)
    };
    (
        skills(addons)
            .iter()
            .filter(|(_, skill)| owned(&skill.id))
            .count(),
        addons
            .mcp_servers
            .iter()
            .filter(|server| owned(&server.name))
            .count(),
    )
}
