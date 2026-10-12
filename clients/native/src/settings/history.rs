//! Readable differences between two profile drafts for the history page.

use std::collections::BTreeSet;

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
    if before.addons != after.addons {
        lines.push("Дополнения агента: skills, MCP или пакеты дополнений".to_string());
    }
    lines
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
