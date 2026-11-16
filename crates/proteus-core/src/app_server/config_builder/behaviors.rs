//! Source-owned behavior definitions. A saved catalogue never changes the current selection.
use super::*;
use crate::domain::BehaviorModeDefinition;

type Definitions = BTreeMap<String, BehaviorModeDefinition>;

pub(super) fn proposal<'a>(
    config: &AppConfig,
    modes: Option<&'a Definitions>,
    expected: Option<&Definitions>,
) -> Result<Option<&'a Definitions>> {
    if let Some(expected) = expected {
        anyhow::ensure!(
            modes.is_some(),
            "expected_modes requires a mode catalogue replacement"
        );
        anyhow::ensure!(
            expected == &config.modes,
            "behavior modes changed; refresh settings before saving"
        );
    }
    let changed = modes.filter(|modes| **modes != config.modes);
    anyhow::ensure!(
        changed.is_none() || config.behavior_source.writable,
        "behavior modes are inherited; edit their owning source"
    );
    Ok(changed)
}

fn changed(config: &AppConfig, baseline: Option<&AppConfig>) -> bool {
    baseline.is_none_or(|baseline| config.modes != baseline.modes)
}

pub(super) fn json(
    source: &mut serde_json::Map<String, Value>,
    config: &AppConfig,
    baseline: Option<&AppConfig>,
) -> Result<()> {
    if !changed(config, baseline) {
        return Ok(());
    }
    if config.modes.is_empty() {
        source.remove("modes");
        return Ok(());
    }
    let entries = source
        .entry("modes")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("modes must be an object")?;
    entries.retain(|name, _| config.modes.contains_key(name));
    for (name, definition) in &config.modes {
        if baseline.and_then(|baseline| baseline.modes.get(name)) != Some(definition) {
            entries.insert(name.clone(), serde_json::to_value(definition)?);
        }
    }
    Ok(())
}

pub(super) fn toml(
    source: &mut toml_edit::DocumentMut,
    config: &AppConfig,
    baseline: Option<&AppConfig>,
) -> Result<()> {
    if !changed(config, baseline) {
        return Ok(());
    }
    if config.modes.is_empty() {
        source.remove("modes");
        return Ok(());
    }
    if source.get("modes").is_none() {
        source["modes"] = toml_edit::table();
    }
    let entries = source
        .get_mut("modes")
        .and_then(toml_edit::Item::as_table_like_mut)
        .context("modes must be a table")?;
    let removed = entries
        .iter()
        .filter(|(id, _)| !config.modes.contains_key(*id))
        .map(|(id, _)| id.to_owned())
        .collect::<Vec<_>>();
    for id in removed {
        entries.remove(&id);
    }
    for (name, definition) in &config.modes {
        let old = baseline.and_then(|baseline| baseline.modes.get(name));
        if old == Some(definition) {
            continue;
        }
        if entries.get(name).is_none() {
            entries.insert(
                name,
                toml_edit::Item::Value(
                    toml_edit::Value::InlineTable(toml_edit::InlineTable::new()),
                ),
            );
        }
        let table = entries
            .get_mut(name)
            .and_then(toml_edit::Item::as_table_like_mut)
            .context("mode definition must be a table")?;
        let before = old
            .map(serde_json::to_value)
            .transpose()?
            .unwrap_or_else(|| serde_json::json!({}));
        let after = serde_json::to_value(definition)?;
        for key in ["title", "instructions", "tools", "hooks", "permission_mode"] {
            if before.get(key) == after.get(key) {
                continue;
            }
            let Some(value) = after.get(key) else {
                table.remove(key);
                continue;
            };
            let mut next = match value {
                Value::String(value) => toml_edit::value(value.clone()),
                Value::Array(values) => toml_edit::value(
                    values
                        .iter()
                        .map(|v| v.as_str().unwrap())
                        .collect::<toml_edit::Array>(),
                ),
                _ => unreachable!("behavior definition fields are strings or string lists"),
            };
            if let (Some(old), Some(next)) = (
                table.get(key).and_then(toml_edit::Item::as_value),
                next.as_value_mut(),
            ) {
                *next.decor_mut() = old.decor().clone();
            }
            table.insert(key, next);
        }
    }
    Ok(())
}
