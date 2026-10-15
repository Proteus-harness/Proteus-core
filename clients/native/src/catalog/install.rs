//! Per-kind installation and draft binding. Files of Agent Plugins are
//! immutable versions: unsaved profile edits never remove a saved package.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use proteus_contracts::domain::ConfiguredMcpServerConfig;

use super::{
    Installed, Locations, Source, Target, key,
    schema::{self, Catalog, Entry, Kind},
};
use crate::{packages::Contents, settings::Draft};

pub fn receipt<'a>(
    installed: &'a [Installed],
    source: &Source,
    entry: &Entry,
    profile: &Path,
) -> Option<&'a Installed> {
    installed.iter().find(|item| {
        item.source == *source
            && item.entry == entry.id
            && item.kind == entry.kind
            && (!matches!(entry.kind, Kind::McpServer | Kind::AgentPlugin)
                || item.profile.as_deref() == Some(profile))
    })
}

pub fn connected(installed: &Installed, draft: &Draft) -> bool {
    match &installed.target {
        Target::Mcp { server } => draft.configured_mcp(&server.name).is_some(),
        Target::Folder { path } if installed.kind == Kind::AgentPlugin => draft
            .addons
            .addons
            .plugins
            .iter()
            .any(|plugin| plugin.path == *path),
        Target::Folder { path } => path.is_dir(),
    }
}

/// Runs on a worker thread. No shell, builds or install scripts are run.
pub fn prepare(
    locations: &Locations,
    source: &Source,
    catalog: &Catalog,
    entry: &Entry,
    previous: Option<&Installed>,
    profile: &Path,
    taken_extensions: &[String],
) -> Result<Installed> {
    let dir = schema::directory(&catalog.root, entry)?;
    let target = match entry.kind {
        Kind::McpServer => {
            let files = Contents::folder(&dir)?;
            let server: ConfiguredMcpServerConfig =
                serde_json::from_str(files.text("server.json")?)?;
            ensure!(
                !server.name.trim().is_empty() && !server.command.trim().is_empty(),
                "Нужны имя и команда MCP-сервера"
            );
            ensure!(
                server.cwd.as_ref().is_none_or(|path| path.is_absolute()),
                "cwd сервера должен быть абсолютным; для файлов сервера используйте Agent Plugin"
            );
            Target::Mcp { server }
        }
        Kind::ClientExtension | Kind::Skill | Kind::AgentPlugin => {
            let files = Contents::folder(&dir)?;
            let (root, id) = match entry.kind {
                Kind::ClientExtension => {
                    let manifest =
                        crate::declarative::manifest::parse(files.text("extension.json")?)?;
                    ensure!(
                        manifest.id == entry.id,
                        "id каталога должен совпадать с extension.json"
                    );
                    let managed = previous.is_some_and(|old| matches!(&old.target, Target::Folder { path } if *path == locations.extensions.join(&entry.id)));
                    ensure!(
                        managed || !taken_extensions.contains(&entry.id),
                        "Имя расширения занято"
                    );
                    (&locations.extensions, entry.id.clone())
                }
                Kind::Skill => {
                    ensure!(
                        !files.text("SKILL.md")?.trim().is_empty(),
                        "Пустой SKILL.md"
                    );
                    (&locations.skills, entry.id.clone())
                }
                Kind::AgentPlugin => {
                    plugin_name(&files)?;
                    let reusable = previous
                        .filter(|old| old.version == entry.version)
                        .and_then(|old| match &old.target {
                            Target::Folder { path }
                                if path.parent() == Some(locations.plugins.as_path())
                                    && path.is_dir() =>
                            {
                                path.file_name().and_then(|name| name.to_str())
                            }
                            _ => None,
                        });
                    // Discarded drafts may leave an unreferenced version on disk.
                    // A fresh installation never overwrites or adopts it silently.
                    let id = reusable.map(str::to_string).unwrap_or_else(|| {
                        format!(
                            "{}-{}-{}",
                            source.key(),
                            key(&entry.id),
                            uuid::Uuid::new_v4()
                        )
                    });
                    (&locations.plugins, id)
                }
                Kind::McpServer => unreachable!(),
            };
            let path = root.join(&id);
            let managed = previous.is_some_and(
                |old| matches!(&old.target, Target::Folder { path: old } if *old == path),
            );
            // An existing immutable plugin version may only be reused by its receipt.
            if !(entry.kind == Kind::AgentPlugin && managed && path.exists()) {
                files.install(root, &id, managed)?;
            }
            Target::Folder { path }
        }
    };
    Ok(Installed {
        source: source.clone(),
        entry: entry.id.clone(),
        version: entry.version.clone(),
        kind: entry.kind,
        profile: matches!(entry.kind, Kind::McpServer | Kind::AgentPlugin)
            .then(|| profile.to_owned()),
        target,
    })
}

/// Bind the prepared result to the current draft. Name collisions never
/// replace an independently configured server.
pub fn connect(
    draft: &mut Draft,
    installed: &Installed,
    previous: Option<&Installed>,
) -> Result<()> {
    match &installed.target {
        Target::Folder { path } if installed.kind == Kind::AgentPlugin => {
            if let Some(Installed {
                target: Target::Folder { path: old },
                ..
            }) = previous
            {
                draft.remove_package(old);
            }
            draft.add_package(path.clone());
        }
        Target::Mcp { server } => {
            let old = previous.and_then(|old| match &old.target {
                Target::Mcp { server } => Some(server),
                _ => None,
            });
            if let Some(old) = old
                && let Some(current) = draft.configured_mcp(&old.name)
            {
                ensure!(
                    current == old,
                    "Сервер изменён вручную; сначала удалите его на странице MCP"
                );
            }
            if let Some(current) = draft.configured_mcp(&server.name) {
                ensure!(
                    old.is_some_and(|old| current == old),
                    "Имя MCP-сервера уже занято: {}",
                    server.name
                );
            }
            let disabled = old.is_some_and(|old| !draft.mcp_enabled(&old.name));
            if let Some(old) = old {
                if let Some(ix) = draft
                    .addons
                    .mcp_servers
                    .iter()
                    .position(|item| item.name == old.name)
                {
                    draft.remove_mcp(ix);
                }
            }
            draft.addons.mcp_servers.push(server.clone());
            if disabled {
                draft.set_mcp(&server.name, false);
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn disconnect(draft: &mut Draft, installed: &Installed) -> Result<()> {
    match &installed.target {
        Target::Mcp { server } => {
            if let Some(ix) = draft
                .addons
                .mcp_servers
                .iter()
                .position(|item| item.name == server.name)
            {
                ensure!(
                    draft.addons.mcp_servers[ix] == *server,
                    "Сервер изменён вручную; удалите его на странице MCP"
                );
                draft.remove_mcp(ix);
            }
        }
        Target::Folder { path } if installed.kind == Kind::AgentPlugin => {
            draft.remove_package(path)
        }
        _ => {}
    }
    Ok(())
}

/// Only immediate, receipted children of the applicable managed root.
pub fn remove_files(locations: &Locations, installed: &Installed) -> Result<()> {
    let Target::Folder { path } = &installed.target else {
        return Ok(());
    };
    let root = match installed.kind {
        Kind::ClientExtension => &locations.extensions,
        Kind::Skill => &locations.skills,
        // Profile references may exist elsewhere; keep immutable plugin versions.
        _ => return Ok(()),
    };
    ensure!(
        path.parent() == Some(root.as_path()),
        "Установка находится вне управляемой папки"
    );
    if path.exists() {
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}

/// The server remains the authoritative Agent Plugins validator. Here we
/// check the manifest required to accept a local archive before writing it.
fn plugin_name(files: &Contents) -> Result<String> {
    let value: serde_json::Value = serde_json::from_str(files.text("plugin.json")?)?;
    ensure!(
        value["$schema"] == "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
        "Нужен Agent Plugins 1.0 plugin.json"
    );
    let name = value["name"].as_str().context("В plugin.json нужен name")?;
    ensure!(
        !name.is_empty()
            && name.len() <= 64
            && !name.contains("..")
            && !name.contains("--")
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
            && name.as_bytes()[0].is_ascii_alphanumeric()
            && name.as_bytes()[name.len() - 1].is_ascii_alphanumeric(),
        "Неверное имя Agent Plugin"
    );
    Ok(name.to_owned())
}

pub fn import_zip(locations: &Locations, path: &Path) -> Result<PathBuf> {
    let files = Contents::zip(path)?;
    plugin_name(&files)?;
    files.install(
        &locations.plugins,
        &format!("zip-{}", uuid::Uuid::new_v4()),
        false,
    )
}
