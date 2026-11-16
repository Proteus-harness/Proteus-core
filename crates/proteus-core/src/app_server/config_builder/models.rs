mod persistence;
use super::*;
use crate::core::{ModelSourceEdits, ProviderProfileConfig};
pub(super) use persistence::{json_presets, json_roles, toml_presets, toml_roles};

/// Prefer exact source intent unless a typed caller deliberately changed the
/// resolved selection after loading (the existing persistence helper contract).
pub(super) fn source_choice(config: &AppConfig) -> Option<String> {
    if config.active_provider == config.model_source.resolved_choice {
        if let Some(choice) = &config.model_source.choice {
            return choice.clone();
        }
    }
    config.active_provider.clone()
}

pub(super) fn choice_writable(config: &AppConfig) -> bool {
    config.model_source.choice.is_none() || config.model_source.choice_writable
}

pub(super) fn presets(config: &AppConfig) -> BTreeMap<String, ConfigBuilderModelPreset> {
    config
        .providers
        .iter()
        .map(|(name, preset)| {
            (
                name.clone(),
                ConfigBuilderModelPreset {
                    provider: preset.provider.clone(),
                    model: preset.model.clone(),
                    stream: preset.stream,
                    reasoning: preset.reasoning.clone(),
                    reasoning_efforts: preset.reasoning_efforts.clone(),
                },
            )
        })
        .collect()
}

pub(super) fn roles(config: &AppConfig) -> Vec<ConfigBuilderModelRole> {
    config
        .model_source
        .roles
        .iter()
        .map(|role| ConfigBuilderModelRole {
            name: role.name.clone(),
            model: role.model.clone(),
            editable: role.editable,
            description: role.description.clone(),
        })
        .collect()
}

pub(super) fn proposal(
    baseline: &AppConfig,
    selected: Option<&Option<String>>,
    models: Option<&BTreeMap<String, ConfigBuilderModelPreset>>,
    role_models: Option<&BTreeMap<String, Option<String>>>,
) -> Result<Option<ModelSourceEdits>> {
    let current = presets(baseline);
    if selected.is_some() {
        anyhow::ensure!(
            choice_writable(baseline),
            "model selection is read-only in this source"
        );
    }
    if let Some(Some(name)) = selected {
        anyhow::ensure!(!name.trim().is_empty(), "model alias must not be empty");
        anyhow::ensure!(
            models.unwrap_or(&current).contains_key(name),
            "model alias {name:?} is not defined"
        );
    }
    let presets = models
        .filter(|models| **models != current)
        .map(|models| {
            models
                .iter()
                .map(|(name, preset)| {
                    anyhow::ensure!(!name.trim().is_empty(), "model aliases must not be empty");
                    anyhow::ensure!(
                        !preset.provider.trim().is_empty() && !preset.model.trim().is_empty(),
                        "model {name:?} needs a connection and model ID"
                    );
                    anyhow::ensure!(
                        preset
                            .reasoning
                            .effort
                            .as_ref()
                            .is_none_or(|v| !v.trim().is_empty()),
                        "model {name:?} has an empty effort"
                    );
                    Ok((
                        name.clone(),
                        ProviderProfileConfig {
                            provider: preset.provider.clone(),
                            model: preset.model.clone(),
                            stream: preset.stream,
                            reasoning: preset.reasoning.clone(),
                            reasoning_efforts: preset.reasoning_efforts.clone(),
                        },
                    ))
                })
                .collect::<Result<BTreeMap<_, _>>>()
        })
        .transpose()?;
    let selected = selected
        .filter(|selected| **selected != source_choice(baseline))
        .cloned();
    let roles = role_models
        .into_iter()
        .flatten()
        .filter(|(name, value)| {
            baseline
                .model_source
                .roles
                .iter()
                .find(|role| &role.name == *name)
                .is_none_or(|role| &role.model != *value)
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    if presets.is_none() && selected.is_none() && roles.is_empty() {
        return Ok(None);
    }
    Ok(Some(ModelSourceEdits {
        selected,
        presets,
        roles,
    }))
}
