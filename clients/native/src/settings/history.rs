//! Readable differences between two profile drafts for the history page.

use std::collections::BTreeSet;

use proteus_contracts::app_protocol::addons::AppAddonsUpdate;
use proteus_contracts::app_protocol::config_builder::ConfigBuilderSnapshot;
use serde_json::Value;

use super::catalog::slot_title;
use super::draft::Draft;

/// What changed from `before` to `after`, one line per profile area.
pub fn describe_changes(
    snapshot: &ConfigBuilderSnapshot,
    before: &Draft,
    after: &Draft,
) -> Vec<String> {
    let mut lines = Vec::new();
    if before.mode != after.mode {
        lines.push(format!("permission_mode: {} → {}", before.mode, after.mode));
    }
    if before.provider != after.provider {
        lines.push(format!(
            "Model: {} → {}",
            before.provider.as_deref().unwrap_or("нет"),
            after.provider.as_deref().unwrap_or("нет")
        ));
    }
    let tools: Vec<String> = after
        .tools
        .difference(&before.tools)
        .map(|name| format!("+ {name}"))
        .chain(
            before
                .tools
                .difference(&after.tools)
                .map(|name| format!("− {name}")),
        )
        .collect();
    if !tools.is_empty() {
        lines.push(format!("Tools: {}", tools.join(", ")));
    }
    if before.hooks != after.hooks {
        lines.push(format!(
            "Hooks: {} → {}",
            list(&before.hooks),
            list(&after.hooks)
        ));
    }
    let slots: BTreeSet<&String> = before
        .modules
        .keys()
        .chain(after.modules.keys())
        .chain(before.config.keys())
        .chain(after.config.keys())
        .collect();
    for slot in slots {
        let mut parts = Vec::new();
        let (from, to) = (before.modules.get(slot), after.modules.get(slot));
        if from != to {
            parts.push(format!(
                "{} → {}",
                from.map(String::as_str).unwrap_or("нет"),
                to.map(String::as_str).unwrap_or("нет")
            ));
        }
        let keys = changed_keys(before, after, slot);
        if !keys.is_empty() {
            parts.push(format!(
                "параметры: {}",
                keys.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        if !parts.is_empty() {
            lines.push(format!(
                "{}: {}",
                slot_title(snapshot, slot),
                parts.join("; ")
            ));
        }
    }
    lines.extend(addon_changes(&before.addons, &after.addons));
    lines
}

fn addon_changes(before: &AppAddonsUpdate, after: &AppAddonsUpdate) -> Vec<String> {
    let mut lines = Vec::new();
    let skills = switched(
        &before.addons.disabled_skills,
        &after.addons.disabled_skills,
    );
    if !skills.is_empty() {
        lines.push(format!("Skills: {}", skills.join(", ")));
    }
    let mut servers = switched(
        &before.addons.disabled_mcp_servers,
        &after.addons.disabled_mcp_servers,
    );
    servers.extend(entry_changes(
        &before.mcp_servers,
        &after.mcp_servers,
        |server| server.name.clone(),
        |server| server.enabled,
    ));
    if !servers.is_empty() {
        lines.push(format!("MCP: {}", servers.join(", ")));
    }
    let packages = entry_changes(
        &before.addons.plugins,
        &after.addons.plugins,
        |plugin| plugin.path.display().to_string(),
        |plugin| plugin.enabled,
    );
    if !packages.is_empty() {
        lines.push(format!("Packages: {}", packages.join(", ")));
    }
    lines
}

/// Entries leaving a disabled list are switched on (`+`), new ones off (`−`).
fn switched(before: &[String], after: &[String]) -> Vec<String> {
    before
        .iter()
        .filter(|item| !after.contains(item))
        .map(|item| format!("+ {item}"))
        .chain(
            after
                .iter()
                .filter(|item| !before.contains(item))
                .map(|item| format!("− {item}")),
        )
        .collect()
}

/// Added, removed, switched and otherwise edited entries matched by key.
fn entry_changes<'a, T: PartialEq>(
    before: &'a [T],
    after: &'a [T],
    key: impl Fn(&T) -> String,
    enabled: impl Fn(&T) -> bool,
) -> Vec<String> {
    let find = |list: &'a [T], name: &str| list.iter().find(|entry| key(entry) == name);
    let mut changes: Vec<String> = after
        .iter()
        .filter_map(|entry| {
            let name = key(entry);
            match find(before, &name) {
                None => Some(format!("добавлен {name}")),
                Some(old) if enabled(old) != enabled(entry) => {
                    Some(format!("{} {name}", if enabled(entry) { "+" } else { "−" }))
                }
                Some(old) if old != entry => Some(format!("изменён {name}")),
                Some(_) => None,
            }
        })
        .collect();
    changes.extend(
        before
            .iter()
            .map(&key)
            .filter(|name| find(after, name).is_none())
            .map(|name| format!("удалён {name}")),
    );
    changes
}

fn list(items: &[String]) -> String {
    if items.is_empty() {
        "нет".to_string()
    } else {
        items.join(", ")
    }
}

fn changed_keys(before: &Draft, after: &Draft, slot: &str) -> BTreeSet<String> {
    let empty = Default::default();
    let (from, to) = (
        before.config.get(slot).unwrap_or(&empty),
        after.config.get(slot).unwrap_or(&empty),
    );
    let mut keys = BTreeSet::new();
    for module in from.keys().chain(to.keys()) {
        let (old, new) = (object(from.get(module)), object(to.get(module)));
        for key in old.keys().chain(new.keys()) {
            if old.get(key) != new.get(key) {
                keys.insert(key.clone());
            }
        }
    }
    keys
}

fn object(value: Option<&Value>) -> serde_json::Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}
