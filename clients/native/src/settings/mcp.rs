//! Profile MCP servers of the draft (`tools.mcp_servers`): adding, editing
//! and removing entries, their text form and the checks that block saving.

use std::collections::BTreeMap;
use std::path::PathBuf;

use proteus_contracts::app_protocol::addons::AppAddonsSnapshot;
use proteus_contracts::domain::{ConfiguredMcpServerConfig, ToolSafety, mcp_protocol_version};
use serde_json::json;

use super::addons::{package_servers, set_listed};
use super::draft::Draft;

/// Text fields of a profile server's form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum McpText {
    Name,
    Command,
    Args,
    Cwd,
    Env,
    EnvAllowlist,
    TimeoutMs,
    MaxResponseBytes,
    ProtocolVersion,
}

impl McpText {
    pub const ALL: [McpText; 9] = [
        Self::Name,
        Self::Command,
        Self::Args,
        Self::Cwd,
        Self::Env,
        Self::EnvAllowlist,
        Self::TimeoutMs,
        Self::MaxResponseBytes,
        Self::ProtocolVersion,
    ];

    /// Fields edited one entry per line.
    pub fn is_multiline(self) -> bool {
        matches!(self, Self::Args | Self::Env | Self::EnvAllowlist)
    }
}

/// Safety classes of the menu with their titles.
pub const SAFETY_TITLES: [(ToolSafety, &str); 5] = [
    (ToolSafety::ReadOnly, "Только чтение"),
    (ToolSafety::WritesFiles, "Пишет файлы"),
    (ToolSafety::RunsCommands, "Запускает команды"),
    (ToolSafety::Network, "Сеть"),
    (ToolSafety::Dangerous, "Опасные действия"),
];

pub fn safety_title(safety: &ToolSafety) -> &'static str {
    SAFETY_TITLES
        .iter()
        .find(|(item, _)| item == safety)
        .map_or("", |(_, title)| *title)
}

/// Why a profile server cannot be saved, attached to the field to fix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpProblem {
    pub index: usize,
    pub field: McpText,
    pub message: &'static str,
}

impl Draft {
    /// Appends a server with a free `server-N` name and no command yet;
    /// returns its position.
    pub fn add_mcp(&mut self) -> usize {
        let servers = &mut self.addons.mcp_servers;
        let taken = |name: &str| servers.iter().any(|server| server.name == name);
        let name = std::iter::once("server".to_string())
            .chain((2..).map(|n| format!("server-{n}")))
            .find(|name| !taken(name))
            .expect("names are endless");
        // Defaults as for a profile table that names only these two keys.
        let server = serde_json::from_value(json!({ "name": name, "command": "" }))
            .expect("name and command make a complete server entry");
        servers.push(server);
        servers.len() - 1
    }

    /// Removes the server at `ix` together with its `disabled_mcp_servers`
    /// entry, unless another profile server still uses that name.
    pub fn remove_mcp(&mut self, ix: usize) {
        if ix >= self.addons.mcp_servers.len() {
            return;
        }
        let removed = self.addons.mcp_servers.remove(ix);
        if !self.profile_mcp_named(&removed.name) {
            set_listed(
                &mut self.addons.addons.disabled_mcp_servers,
                &removed.name,
                false,
            );
        }
    }

    pub fn edit_mcp(&mut self, ix: usize, edit: impl FnOnce(&mut ConfiguredMcpServerConfig)) {
        if let Some(server) = self.addons.mcp_servers.get_mut(ix) {
            edit(server);
        }
    }

    /// Applies the text of one form field; text that does not parse leaves
    /// the server as it was. A new name carries a `disabled_mcp_servers`
    /// entry of the old one with it.
    pub fn set_mcp_text(&mut self, ix: usize, field: McpText, text: &str) -> Result<(), String> {
        let Some(server) = self.addons.mcp_servers.get_mut(ix) else {
            return Ok(());
        };
        match field {
            McpText::Name => {
                let old = std::mem::replace(&mut server.name, text.trim().to_string());
                let new = server.name.clone();
                self.follow_rename(&old, &new);
            }
            McpText::Command => server.command = text.trim().to_string(),
            McpText::Args => server.args = parse_lines(text),
            McpText::Cwd => {
                let text = text.trim();
                server.cwd = (!text.is_empty()).then(|| PathBuf::from(text));
            }
            McpText::Env => server.environment.env = parse_env(text)?,
            McpText::EnvAllowlist => server.environment.env_allowlist = parse_lines(text),
            McpText::TimeoutMs => server.timeout_ms = parse_limit(text)?,
            McpText::MaxResponseBytes => {
                server.max_response_bytes = parse_limit(text)?
                    .map(usize::try_from)
                    .transpose()
                    .map_err(|_| "Слишком большое число".to_string())?;
            }
            McpText::ProtocolVersion => {
                let text = text.trim();
                server.protocol_version = if text.is_empty() {
                    mcp_protocol_version()
                } else {
                    text.to_string()
                };
            }
        }
        Ok(())
    }

    /// Saved entry that the draft server at `ix` edits: the one with its
    /// name, or the one at its position when that name left the draft, as
    /// after renaming.
    pub fn mcp_baseline<'a>(
        &self,
        baseline: &'a Draft,
        ix: usize,
    ) -> Option<&'a ConfiguredMcpServerConfig> {
        let server = self.addons.mcp_servers.get(ix)?;
        let saved = &baseline.addons.mcp_servers;
        saved
            .iter()
            .find(|saved| saved.name == server.name)
            .or_else(|| {
                saved
                    .get(ix)
                    .filter(|saved| !self.profile_mcp_named(&saved.name))
            })
    }

    fn profile_mcp_named(&self, name: &str) -> bool {
        self.addons
            .mcp_servers
            .iter()
            .any(|server| server.name == name)
    }

    fn follow_rename(&mut self, old: &str, new: &str) {
        // Another server keeps the old name, and its switch with it.
        if old == new || self.profile_mcp_named(old) {
            return;
        }
        let disabled = &mut self.addons.addons.disabled_mcp_servers;
        if disabled.iter().any(|name| name == old) {
            set_listed(disabled, old, false);
            set_listed(disabled, new, true);
        }
    }
}

/// Text of one form field as the editor shows it.
pub fn mcp_text(server: &ConfiguredMcpServerConfig, field: McpText) -> String {
    match field {
        McpText::Name => server.name.clone(),
        McpText::Command => server.command.clone(),
        McpText::Args => server.args.join("\n"),
        McpText::Cwd => server
            .cwd
            .as_ref()
            .map(|cwd| cwd.display().to_string())
            .unwrap_or_default(),
        McpText::Env => server
            .environment
            .env
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("\n"),
        McpText::EnvAllowlist => server.environment.env_allowlist.join("\n"),
        McpText::TimeoutMs => server
            .timeout_ms
            .map(|value| value.to_string())
            .unwrap_or_default(),
        McpText::MaxResponseBytes => server
            .max_response_bytes
            .map(|value| value.to_string())
            .unwrap_or_default(),
        McpText::ProtocolVersion => server.protocol_version.clone(),
    }
}

/// Profile servers without a name or command, or with a name another
/// server already has, including the servers of loaded packages.
pub fn mcp_problems(draft: &Draft, addons: Option<&AppAddonsSnapshot>) -> Vec<McpProblem> {
    let servers = &draft.addons.mcp_servers;
    let packaged: Vec<&str> = addons
        .into_iter()
        .flat_map(package_servers)
        .map(|state| state.name.as_str())
        .collect();
    let mut problems = Vec::new();
    for (index, server) in servers.iter().enumerate() {
        let name = server.name.as_str();
        let taken = servers
            .iter()
            .enumerate()
            .any(|(other, entry)| other != index && entry.name == name)
            || packaged.contains(&name);
        let name_problem = if name.trim().is_empty() {
            Some("Укажите имя")
        } else if taken {
            Some("Имя уже занято")
        } else {
            None
        };
        problems.extend(name_problem.map(|message| McpProblem {
            index,
            field: McpText::Name,
            message,
        }));
        if server.command.trim().is_empty() {
            problems.push(McpProblem {
                index,
                field: McpText::Command,
                message: "Укажите команду",
            });
        }
    }
    problems
}

fn parse_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_env(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut env = BTreeMap::new();
    for (ix, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line
            .split_once('=')
            .filter(|(key, _)| !key.trim().is_empty())
        else {
            return Err(format!("Строка {}: нужно KEY=VALUE", ix + 1));
        };
        let key = key.trim();
        if env
            .insert(key.to_string(), value.trim().to_string())
            .is_some()
        {
            return Err(format!("{key} задана дважды"));
        }
    }
    Ok(env)
}

/// Empty text keeps the runtime default.
fn parse_limit(text: &str) -> Result<Option<u64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<u64>() {
        Ok(value) if value > 0 => Ok(Some(value)),
        _ => Err("Нужно целое число больше нуля".to_string()),
    }
}
