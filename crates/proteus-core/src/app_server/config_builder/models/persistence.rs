//! Preserve unrelated model tables, field decorations and exact role references.
use super::*;

fn changed_roles<'a>(
    config: &'a AppConfig,
    baseline: Option<&'a AppConfig>,
) -> impl Iterator<Item = (&'a crate::core::SourceModelRole, bool)> {
    config.model_source.roles.iter().filter_map(move |role| {
        let previous = baseline.and_then(|baseline| {
            baseline
                .model_source
                .roles
                .iter()
                .find(|old| old.name == role.name)
        });
        (role.editable && previous.is_some_and(|old| old.model != role.model))
            .then(|| (role, previous.unwrap().child_choice))
    })
}

pub(in crate::app_server::config_builder) fn toml_roles(
    document: &mut toml_edit::DocumentMut,
    config: &AppConfig,
    baseline: Option<&AppConfig>,
) -> Result<()> {
    for (role, child_choice) in changed_roles(config, baseline) {
        let entries = document
            .get_mut("agent_control")
            .and_then(|v| v.get_mut("roles"))
            .context("agent role source is unavailable")?;
        let len = entries
            .as_array_of_tables()
            .map(|v| v.len())
            .or_else(|| entries.as_array().map(|v| v.len()))
            .context("agent roles must be an array")?;
        let index = (0..len)
            .find(|&i| {
                entries
                    .get(i)
                    .and_then(|v| v.get("name"))
                    .and_then(toml_edit::Item::as_str)
                    == Some(role.name.as_str())
            })
            .context("agent role source is unavailable")?;
        let entry = entries.get_mut(index).unwrap();
        let target = if child_choice {
            entry
                .get_mut("config")
                .context("inline config source is unavailable")?
        } else {
            entry
        };
        let target = target
            .as_table_like_mut()
            .context("agent role must be an object")?;
        target.remove("active_provider");
        match &role.model {
            Some(alias) => {
                target.insert("model", toml_edit::value(alias.clone()));
            }
            None => {
                target.remove("model");
            }
        }
    }
    Ok(())
}

pub(in crate::app_server::config_builder) fn json_roles(
    source: &mut serde_json::Map<String, Value>,
    config: &AppConfig,
    baseline: Option<&AppConfig>,
) -> Result<()> {
    for (role, child_choice) in changed_roles(config, baseline) {
        let entry = source
            .get_mut("agent_control")
            .and_then(|v| v.get_mut("roles"))
            .and_then(Value::as_array_mut)
            .and_then(|entries| {
                entries.iter_mut().find(|entry| {
                    entry.get("name").and_then(Value::as_str) == Some(role.name.as_str())
                })
            })
            .context("agent role source is unavailable")?;
        let target = if child_choice {
            entry
                .get_mut("config")
                .context("inline config source is unavailable")?
        } else {
            entry
        };
        let target = target
            .as_object_mut()
            .context("agent role must be an object")?;
        target.remove("active_provider");
        match &role.model {
            Some(alias) => {
                target.insert("model".into(), Value::String(alias.clone()));
            }
            None => {
                target.remove("model");
            }
        }
    }
    Ok(())
}

fn assign(table: &mut dyn toml_edit::TableLike, key: &str, mut next: toml_edit::Item) {
    if let (Some(old), Some(next)) = (
        table.get(key).and_then(toml_edit::Item::as_value),
        next.as_value_mut(),
    ) {
        *next.decor_mut() = old.decor().clone();
    }
    table.insert(key, next);
}

fn assign_optional(table: &mut dyn toml_edit::TableLike, key: &str, item: Option<toml_edit::Item>) {
    match item {
        Some(item) => assign(table, key, item),
        None => {
            table.remove(key);
        }
    }
}

pub(in crate::app_server::config_builder) fn toml_presets(
    document: &mut toml_edit::DocumentMut,
    key: &str,
    config: &AppConfig,
    baseline: Option<&AppConfig>,
) -> Result<()> {
    if document.get(key).is_none() {
        document[key] = toml_edit::table();
    }
    let entries = document
        .get_mut(key)
        .and_then(toml_edit::Item::as_table_like_mut)
        .context("model catalogue must be a table")?;
    let deleted = entries
        .iter()
        .filter(|(name, _)| !config.providers.contains_key(*name))
        .map(|(name, _)| name.to_owned())
        .collect::<Vec<_>>();
    for name in deleted {
        entries.remove(&name);
    }
    for (name, preset) in &config.providers {
        let old = baseline.and_then(|b| b.providers.get(name));
        if old == Some(preset) {
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
            .context("model preset must be a table")?;
        for (key, changed, item) in [
            (
                "provider",
                old.is_none_or(|old| old.provider != preset.provider),
                toml_edit::value(preset.provider.clone()),
            ),
            (
                "model",
                old.is_none_or(|old| old.model != preset.model),
                toml_edit::value(preset.model.clone()),
            ),
            (
                "stream",
                old.is_none_or(|old| old.stream != preset.stream),
                toml_edit::value(preset.stream),
            ),
            (
                "reasoning_efforts",
                old.is_none_or(|old| old.reasoning_efforts != preset.reasoning_efforts),
                toml_edit::value(
                    preset
                        .reasoning_efforts
                        .iter()
                        .cloned()
                        .collect::<toml_edit::Array>(),
                ),
            ),
        ] {
            if changed {
                assign(table, key, item);
            }
        }
        let flat = ["effort", "summary", "budget_tokens"]
            .iter()
            .any(|key| table.contains_key(key));
        for (key, changed, item) in [
            (
                "effort",
                old.is_none_or(|old| old.reasoning.effort != preset.reasoning.effort),
                preset.reasoning.effort.clone().map(toml_edit::value),
            ),
            (
                "summary",
                old.is_none_or(|old| old.reasoning.summary != preset.reasoning.summary),
                Some(toml_edit::value(preset.reasoning.summary)),
            ),
            (
                "budget_tokens",
                old.is_none_or(|old| old.reasoning.budget_tokens != preset.reasoning.budget_tokens),
                preset
                    .reasoning
                    .budget_tokens
                    .map(|v| toml_edit::value(i64::from(v))),
            ),
        ] {
            if !changed {
                continue;
            }
            let nested_has_key = table
                .get("reasoning")
                .and_then(|item| item.get(key))
                .is_some();
            let use_flat = table.contains_key(key) || (flat && !nested_has_key);
            if use_flat {
                assign_optional(table, key, item);
            } else {
                if table.get("reasoning").is_none() {
                    table.insert(
                        "reasoning",
                        toml_edit::Item::Value(toml_edit::Value::InlineTable(
                            toml_edit::InlineTable::new(),
                        )),
                    );
                }
                let nested = table
                    .get_mut("reasoning")
                    .and_then(toml_edit::Item::as_table_like_mut)
                    .context("model reasoning must be a table")?;
                assign_optional(nested, key, item);
            }
        }
    }
    Ok(())
}

pub(in crate::app_server::config_builder) fn json_presets(
    source: &mut serde_json::Map<String, Value>,
    key: &str,
    config: &AppConfig,
    baseline: Option<&AppConfig>,
) -> Result<()> {
    let entries = source
        .entry(key)
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("model catalogue must be an object")?;
    entries.retain(|name, _| config.providers.contains_key(name));
    for (name, preset) in &config.providers {
        let old = baseline.and_then(|b| b.providers.get(name));
        if old == Some(preset) {
            continue;
        }
        if old.is_none() || !entries.contains_key(name) {
            entries.insert(name.clone(), serde_json::to_value(preset)?);
            continue;
        }
        let fields = entries
            .get_mut(name)
            .and_then(Value::as_object_mut)
            .context("model preset must be an object")?;
        for (key, changed, value) in [
            (
                "provider",
                old.is_none_or(|old| old.provider != preset.provider),
                Value::String(preset.provider.clone()),
            ),
            (
                "model",
                old.is_none_or(|old| old.model != preset.model),
                Value::String(preset.model.clone()),
            ),
            (
                "stream",
                old.is_none_or(|old| old.stream != preset.stream),
                Value::Bool(preset.stream),
            ),
            (
                "reasoning_efforts",
                old.is_none_or(|old| old.reasoning_efforts != preset.reasoning_efforts),
                serde_json::to_value(&preset.reasoning_efforts)?,
            ),
        ] {
            if changed {
                fields.insert(key.into(), value);
            }
        }
        let flat = ["effort", "summary", "budget_tokens"]
            .iter()
            .any(|key| fields.contains_key(*key));
        for (key, changed, value) in [
            (
                "effort",
                old.is_none_or(|old| old.reasoning.effort != preset.reasoning.effort),
                preset.reasoning.effort.clone().map(Value::String),
            ),
            (
                "summary",
                old.is_none_or(|old| old.reasoning.summary != preset.reasoning.summary),
                Some(Value::Bool(preset.reasoning.summary)),
            ),
            (
                "budget_tokens",
                old.is_none_or(|old| old.reasoning.budget_tokens != preset.reasoning.budget_tokens),
                preset
                    .reasoning
                    .budget_tokens
                    .map(|v| Value::Number(v.into())),
            ),
        ] {
            if !changed {
                continue;
            }
            let nested = fields
                .get("reasoning")
                .and_then(Value::as_object)
                .is_some_and(|table| table.contains_key(key));
            let target = if fields.contains_key(key) || (flat && !nested) {
                &mut *fields
            } else {
                fields
                    .entry("reasoning")
                    .or_insert_with(|| serde_json::json!({}))
                    .as_object_mut()
                    .context("model reasoning must be an object")?
            };
            match value {
                Some(value) => {
                    target.insert(key.into(), value);
                }
                None => {
                    target.remove(key);
                }
            }
        }
    }
    Ok(())
}
