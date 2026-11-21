//! Small source metadata for editing model presets without guessing aliases
//! from their resolved values. Runtime configurations retain no source document.
use super::*;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Default)]
pub struct ModelSourceMetadata {
    pub catalogue_writable: bool,
    pub roles: Vec<SourceModelRole>,
    /// None means no source metadata (programmatically constructed config).
    pub choice: Option<Option<String>>,
    pub resolved_choice: Option<String>,
    pub choice_writable: bool,
}

#[derive(Debug, Clone)]
pub struct SourceModelRole {
    pub name: String,
    pub model: Option<String>,
    pub editable: bool,
    pub description: String,
    pub(crate) child_choice: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ModelSourceEdits {
    pub selected: Option<Option<String>>,
    pub presets: Option<BTreeMap<String, ProviderProfileConfig>>,
    pub roles: BTreeMap<String, Option<String>>,
}

fn catalogue(source: &Value) -> Option<&Map<String, Value>> {
    source
        .get("models")
        .or_else(|| source.get("providers"))
        .and_then(Value::as_object)
}

impl ModelSourceMetadata {
    pub(super) fn capture(
        merged: &Value,
        local: Option<&Value>,
        available: &BTreeMap<String, ProviderProfileConfig>,
        file: bool,
    ) -> Self {
        let mut local_names = local
            .and_then(catalogue)
            .map(|v| v.keys().collect::<Vec<_>>())
            .unwrap_or_default();
        local_names.sort();
        let complete_local = local.is_some_and(|source| source.get("include").is_none())
            && local.and_then(catalogue).is_none_or(|entries| {
                entries.values().all(|entry| {
                    ["provider", "model"].iter().all(|key| {
                        entry
                            .get(*key)
                            .and_then(Value::as_str)
                            .is_some_and(|value| !value.trim().is_empty())
                    })
                })
            });
        let catalogue_writable =
            file && complete_local && local_names == available.keys().collect::<Vec<_>>();
        let local_roles = local
            .and_then(|v| v.pointer("/agent_control/roles"))
            .and_then(Value::as_array);
        let roles = merged
            .pointer("/agent_control/roles")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|role| {
                let name = role
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let owned = file
                    && local_roles.is_some_and(|roles| {
                        roles.iter().any(|local| {
                            local.get("name").and_then(Value::as_str) == Some(name.as_str())
                        })
                    });
                let child = role.get("config").filter(|v| v.is_object());
                let external = child.is_none();
                let own_catalogue = child.and_then(catalogue).is_some();
                let role_choice = role.get("model").filter(|v| !v.is_null());
                let child_choice = child
                    .and_then(|v| v.get("model").or_else(|| v.get("active_provider")))
                    .filter(|v| !v.is_null());
                let choice = role_choice.or(child_choice);
                let complex = choice.is_some_and(|v| !v.is_string());
                let editable = owned && !external && !own_catalogue && !complex;
                let description = if !owned {
                    "Настройка роли унаследована из другого файла."
                } else if external {
                    "Роль использует отдельный конфиг; настройте её модель в этом файле."
                } else if own_catalogue {
                    "У роли собственный каталог моделей; настройте его в конфиге роли."
                } else if complex {
                    "У роли собственное полное определение модели."
                } else {
                    "Без явного выбора роль наследует текущую модель и effort родителя."
                }
                .to_owned();
                SourceModelRole {
                    name,
                    model: choice.and_then(Value::as_str).map(str::to_owned),
                    editable,
                    description,
                    child_choice: role_choice.is_none() && child_choice.is_some(),
                }
            })
            .collect();
        Self {
            catalogue_writable,
            roles,
            choice: None,
            resolved_choice: None,
            choice_writable: true,
        }
    }

    pub(crate) fn apply(&self, source: &mut Value, edits: &ModelSourceEdits) -> Result<()> {
        let object = source.as_object_mut().context("config must be an object")?;
        if let Some(presets) = &edits.presets {
            anyhow::ensure!(
                self.catalogue_writable,
                "model catalogue is inherited; edit its owning source"
            );
            let key = if object.contains_key("providers") {
                "providers"
            } else {
                "models"
            };
            object.insert(key.into(), serde_json::to_value(presets)?);
        }
        if let Some(selected) = &edits.selected {
            if let Some(selected) = selected {
                anyhow::ensure!(!selected.trim().is_empty(), "model alias must not be empty");
                let key = if object.contains_key("active_provider") {
                    "active_provider"
                } else {
                    "model"
                };
                object.insert(key.into(), Value::String(selected.clone()));
            } else {
                object.remove("model");
                object.remove("active_provider");
            }
        }
        for (name, choice) in &edits.roles {
            let info = self
                .roles
                .iter()
                .find(|r| &r.name == name)
                .with_context(|| format!("unknown model role {name:?}"))?;
            anyhow::ensure!(
                info.editable,
                "model role {name:?} is read-only: {}",
                info.description
            );
            let role = object
                .get_mut("agent_control")
                .and_then(|v| v.get_mut("roles"))
                .and_then(Value::as_array_mut)
                .and_then(|roles| {
                    roles
                        .iter_mut()
                        .find(|r| r.get("name").and_then(Value::as_str) == Some(name))
                })
                .context("model role source is unavailable")?;
            let target = if info.child_choice {
                role.get_mut("config")
                    .context("inline config is unavailable")?
            } else {
                role
            };
            let target = target
                .as_object_mut()
                .context("model role must be an object")?;
            target.remove("active_provider");
            match choice {
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
}

/// Drop the role-level choice when the child editor writes a choice in its
/// config; source normalization deliberately forbids both at the same time.
pub(super) fn remove_outer_choice(source: &mut Value, selection: &[String]) -> Result<()> {
    let Some((name, parents)) = selection.split_last() else {
        return Ok(());
    };
    let parent = super::selected_config_value_mut(source, parents)?;
    let role = parent
        .pointer_mut("/agent_control/roles")
        .and_then(Value::as_array_mut)
        .and_then(|roles| {
            roles
                .iter_mut()
                .find(|role| role.get("name").and_then(Value::as_str) == Some(name.as_str()))
        })
        .context("inline role source is unavailable")?;
    role.as_object_mut()
        .context("inline role must be an object")?
        .remove("model");
    Ok(())
}

/// Only the fields needed to retain ownership and exact aliases survive load.
/// Never retain or duplicate unrelated component settings/instructions here.
pub(super) fn source_view(source: &Value) -> Value {
    let mut view = Map::new();
    for key in ["model", "active_provider", "include"] {
        if let Some(value) = source.get(key) {
            view.insert(key.into(), value.clone());
        }
    }
    for key in ["models", "providers"] {
        if let Some(entries) = source.get(key).and_then(Value::as_object) {
            view.insert(
                key.into(),
                Value::Object(
                    entries
                        .iter()
                        .map(|(name, preset)| {
                            let fields = ["provider", "model"]
                                .into_iter()
                                .filter_map(|key| {
                                    preset.get(key).map(|value| (key.into(), value.clone()))
                                })
                                .collect();
                            (name.clone(), Value::Object(fields))
                        })
                        .collect(),
                ),
            );
        }
    }
    if let Some(roles) = source
        .pointer("/agent_control/roles")
        .and_then(Value::as_array)
    {
        view.insert("agent_control".into(), serde_json::json!({"roles": roles.iter().map(|role| {
            let mut projected = Map::new();
            for key in ["name","model"] { if let Some(value) = role.get(key) { projected.insert(key.into(),value.clone()); } }
            if let Some(child) = role.get("config") { projected.insert("config".into(), if child.is_object() { source_view(child) } else { Value::Null }); }
            Value::Object(projected)
        }).collect::<Vec<_>>()}));
    }
    Value::Object(view)
}

/// The selected child's omitted choice is inheritance, not its resolved boot
/// alias. A role-level string is an exact choice; a full model object is opaque.
pub(super) fn source_choice(
    source: &mut Value,
    selection: &[String],
) -> Result<(Option<String>, bool)> {
    let selected = super::selected_config_value_mut(source, selection)?;
    let choice = selected
        .get("model")
        .or_else(|| selected.get("active_provider"))
        .filter(|value| !value.is_null());
    if let Some(choice) = choice {
        return Ok((choice.as_str().map(str::to_owned), choice.is_string()));
    }
    let Some((name, parents)) = selection.split_last() else {
        return Ok((None, true));
    };
    let parent = super::selected_config_value_mut(source, parents)?;
    let choice = parent
        .pointer("/agent_control/roles")
        .and_then(Value::as_array)
        .and_then(|roles| {
            roles
                .iter()
                .find(|role| role.get("name").and_then(Value::as_str) == Some(name.as_str()))
        })
        .and_then(|role| role.get("model"))
        .filter(|value| !value.is_null());
    Ok((
        choice.and_then(Value::as_str).map(str::to_owned),
        choice.is_none_or(Value::is_string),
    ))
}
