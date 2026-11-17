//! Source-level model names resolve once, before inline role selection.
//! Only the model catalogue and model exports are shared; no child tool or
//! permission configuration is inherited from its parent.
use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value};

use super::{ModelConfig, ProviderProfileConfig};

#[derive(Clone, Default)]
struct ModelScope {
    presets: BTreeMap<String, ProviderProfileConfig>,
    selected: Option<String>,
    components: Map<String, Value>,
    settings: Map<String, Value>,
}

impl ModelScope {
    fn selected_model(&self) -> Result<Option<ModelConfig>> {
        self.selected
            .as_ref()
            .map(|name| {
                self.presets
                    .get(name)
                    .with_context(|| format!("unknown model alias {name:?}"))?
                    .to_model_config()
            })
            .transpose()
    }
}

pub(super) fn normalize_model_sources(source: &mut Value) -> Result<()> {
    normalize(source, None).map(|_| ())
}

fn named_presets(value: Value) -> Result<BTreeMap<String, ProviderProfileConfig>> {
    let entries = value.as_object().context("models must be an object")?;
    let mut result = BTreeMap::new();
    for (name, profile) in entries {
        ensure!(!name.trim().is_empty(), "model aliases must not be empty");
        let mut profile = profile
            .as_object()
            .with_context(|| format!("model alias {name:?} must be an object"))?
            .clone();
        for key in ["provider", "model"] {
            ensure!(
                profile
                    .get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty()),
                "model alias {name:?} must set a nonempty {key}"
            );
        }
        let mut reasoning = match profile.remove("reasoning") {
            Some(value) => value
                .as_object()
                .context("model reasoning must be an object")?
                .clone(),
            None => Map::new(),
        };
        for key in ["effort", "summary", "budget_tokens"] {
            if let Some(value) = profile.remove(key) {
                ensure!(
                    !reasoning.contains_key(key),
                    "model alias {name:?} sets {key} twice"
                );
                reasoning.insert(key.into(), value);
            }
        }
        if !reasoning.is_empty() {
            profile.insert("reasoning".into(), Value::Object(reasoning));
        }
        let profile: ProviderProfileConfig = serde_json::from_value(Value::Object(profile))
            .with_context(|| format!("invalid model alias {name:?}"))?;
        if let Some(effort) = &profile.reasoning.effort {
            ensure!(
                !effort.trim().is_empty(),
                "model alias {name:?} has an empty effort"
            );
        }
        result.insert(name.clone(), profile);
    }
    Ok(result)
}

fn explicit_selection(value: &Value) -> bool {
    value.get("model").is_some_and(|value| !value.is_null())
        || value
            .get("active_provider")
            .is_some_and(|value| !value.is_null())
}

fn normalize(source: &mut Value, parent: Option<&ModelScope>) -> Result<ModelScope> {
    let object = source.as_object_mut().context("config must be an object")?;
    let explicit = object.get("model").is_some_and(|value| !value.is_null())
        || object
            .get("active_provider")
            .is_some_and(|value| !value.is_null());
    ensure!(
        !(object.contains_key("model") && object.contains_key("active_provider")),
        "config must choose model or active_provider, not both"
    );
    ensure!(
        !(object.contains_key("models") && object.contains_key("providers")),
        "config must define models or providers, not both"
    );
    let legacy_independent = object.contains_key("providers") && explicit;
    let mut scope = if legacy_independent {
        ModelScope::default()
    } else {
        parent.cloned().unwrap_or_default()
    };
    let local = if let Some(models) = object.remove("models") {
        named_presets(models)?
    } else if let Some(providers) = object.remove("providers") {
        let providers: BTreeMap<String, ProviderProfileConfig> =
            serde_json::from_value(providers).context("invalid providers catalogue")?;
        for name in providers.keys() {
            ensure!(!name.trim().is_empty(), "model aliases must not be empty");
        }
        providers
    } else {
        BTreeMap::new()
    };
    scope.presets.extend(local);
    if let Some(model) = object.remove("model").filter(|model| !model.is_null()) {
        let name = model.as_str().context("model must be an alias string")?;
        ensure!(!name.trim().is_empty(), "model alias must not be empty");
        scope.selected = Some(name.to_owned());
    } else if object
        .get("active_provider")
        .is_some_and(|value| !value.is_null())
    {
        scope.selected = object["active_provider"].as_str().map(str::to_owned);
    }
    if let Some(name) = &scope.selected {
        ensure!(
            scope.presets.contains_key(name),
            "unknown model alias {name:?}"
        );
        object.insert("active_provider".into(), Value::String(name.clone()));
    }
    if !scope.presets.is_empty() {
        object.insert("providers".into(), serde_json::to_value(&scope.presets)?);
    }
    // Parent transport declarations are shared only when using its catalogue.
    // An explicit legacy child remains a self-contained model configuration.
    if !legacy_independent {
        if let Some(parent) = parent {
            inherit_bindings(object, parent, &scope.presets)?;
        }
    }
    capture_bindings(object, &mut scope);
    let selected = scope.selected_model()?;
    let Some(control) = object
        .get_mut("agent_control")
        .and_then(Value::as_object_mut)
    else {
        return Ok(scope);
    };
    let has_roles = control
        .get("roles")
        .and_then(Value::as_array)
        .is_some_and(|roles| !roles.is_empty());
    if !has_roles {
        return Ok(scope);
    }
    control.insert(
        "model_presets".into(),
        serde_json::to_value(&scope.presets)?,
    );
    control.insert("model_defaults".into(), serde_json::to_value(selected)?);
    let roles = control["roles"]
        .as_array_mut()
        .context("agent_control.roles must be an array")?;
    for role in roles {
        let role = role
            .as_object_mut()
            .context("agent role must be an object")?;
        let choice = role.remove("model");
        let role_explicit = choice.as_ref().is_some_and(|value| !value.is_null());
        let role_choice = match choice {
            Some(Value::String(name)) => Some(name),
            Some(Value::Null) | None => None,
            Some(value) => {
                let model: ModelConfig =
                    serde_json::from_value(value).context("invalid explicit agent model")?;
                role.insert("model".into(), serde_json::to_value(model)?);
                None
            }
        };
        if let Some(child) = role.get_mut("config").filter(|config| config.is_object()) {
            let child_explicit = explicit_selection(child);
            ensure!(
                !(role_explicit && child_explicit),
                "agent role must choose model either on the role or in its config"
            );
            if let Some(name) = &role_choice {
                child
                    .as_object_mut()
                    .unwrap()
                    .insert("model".into(), Value::String(name.clone()));
            }
            let child_scope = normalize(child, Some(&scope))?;
            if child_explicit || role_choice.is_some() {
                role.insert(
                    "model".into(),
                    serde_json::to_value(child_scope.selected_model()?)?,
                );
            }
        } else if let Some(name) = role_choice {
            let model = scope
                .presets
                .get(&name)
                .with_context(|| format!("unknown model alias {name:?}"))?
                .to_model_config()?;
            role.insert("model".into(), serde_json::to_value(model)?);
        }
    }
    Ok(scope)
}

fn capture_bindings(object: &Map<String, Value>, scope: &mut ModelScope) {
    scope.components.clear();
    for (name, component) in object
        .get("components")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        if let Some(models) = component
            .get("exports")
            .and_then(|exports| exports.get("model"))
        {
            let mut component = component.clone();
            component
                .as_object_mut()
                .unwrap()
                .insert("exports".into(), serde_json::json!({"model": models}));
            scope.components.insert(name.clone(), component);
        }
    }
    scope.settings = object
        .get("module_config")
        .and_then(|config| config.get("model"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
}

fn inherit_bindings(
    object: &mut Map<String, Value>,
    parent: &ModelScope,
    presets: &BTreeMap<String, ProviderProfileConfig>,
) -> Result<()> {
    let wanted = presets
        .values()
        .map(|profile| profile.provider.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut components = object
        .remove("components")
        .map(|value| {
            value
                .as_object()
                .cloned()
                .context("components must be an object")
        })
        .transpose()?
        .unwrap_or_default();
    for (name, inherited) in &parent.components {
        let models = inherited["exports"]["model"]
            .as_object()
            .context("model exports must be an object")?;
        for (id, export) in models.iter().filter(|(id, _)| wanted.contains(id.as_str())) {
            let already_bound = components.values().any(|component| {
                component
                    .get("exports")
                    .and_then(|exports| exports.get("model"))
                    .and_then(|models| models.get(id))
                    .is_some()
            });
            if already_bound {
                continue;
            }
            let target = components.entry(name.clone()).or_insert_with(|| {
                let mut target = inherited.clone();
                target["exports"]["model"] = serde_json::json!({});
                target
            });
            let mut target_launch = target.clone();
            target_launch
                .as_object_mut()
                .context("component must be an object")?
                .remove("exports");
            let mut inherited_launch = inherited.clone();
            inherited_launch.as_object_mut().unwrap().remove("exports");
            ensure!(
                target_launch == inherited_launch,
                "conflicting model component binding {name:?}"
            );
            let exports = target
                .as_object_mut()
                .unwrap()
                .entry("exports")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .context("component exports must be an object")?;
            exports
                .entry("model")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .context("model exports must be an object")?
                .insert(id.clone(), export.clone());
        }
    }
    if !components.is_empty() {
        object.insert("components".into(), Value::Object(components));
    }
    if !parent.settings.is_empty() {
        let module_config = object
            .entry("module_config")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .context("module_config must be an object")?;
        let settings = module_config
            .entry("model")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .context("model settings must be an object")?;
        for (id, value) in &parent.settings {
            if wanted.contains(id.as_str()) {
                settings.entry(id.clone()).or_insert_with(|| value.clone());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "models_tests.rs"]
mod tests;
