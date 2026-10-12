//! What the session settings icons choose from: permission modes, models and
//! reasoning efforts of a session config, under the server's names.

use proteus_contracts::app_protocol::config::ConfigSummary;
use proteus_contracts::domain::PermissionMode;

pub(in crate::ui) const MODES: [(PermissionMode, &str); 3] = [
    (PermissionMode::Normal, "normal"),
    (PermissionMode::Auto, "auto"),
    (PermissionMode::Plan, "plan"),
];

/// Name of a persisted permission mode string.
pub(in crate::ui) fn mode_label(value: &str) -> &'static str {
    let mode = mode_from_value(value);
    MODES
        .iter()
        .find(|(candidate, _)| *candidate == mode)
        .map_or("normal", |(_, label)| label)
}

pub(in crate::ui) fn mode_from_value(value: &str) -> PermissionMode {
    match value.to_ascii_lowercase().as_str() {
        "plan" => PermissionMode::Plan,
        "auto" => PermissionMode::Auto,
        _ => PermissionMode::Normal,
    }
}

/// Effort override sent to the server: `None` keeps the profile default.
pub(in crate::ui) fn current_effort(config: &ConfigSummary) -> Option<String> {
    config
        .reasoning
        .effort
        .clone()
        .or_else(|| (!config.reasoning.enabled).then(|| "none".to_owned()))
}

/// Efforts the session can switch between, weakest first as the server
/// lists them.
pub(in crate::ui) fn efforts(config: &ConfigSummary) -> &[String] {
    &config.reasoning.effort_options
}

/// Effort the profile falls back to when the session has no override.
pub(in crate::ui) fn default_effort(config: &ConfigSummary) -> Option<&str> {
    let current = config.model.as_ref()?;
    config
        .model_options
        .iter()
        .find(|option| option.name == current.name)?
        .default_reasoning_effort
        .as_deref()
}

pub(in crate::ui) fn model_label(config: &ConfigSummary) -> String {
    config
        .model
        .as_ref()
        .map(|model| model.name.clone())
        .unwrap_or_else(|| "model".to_owned())
}

/// Provider every offered model shares, for the heading of the model list.
pub(in crate::ui) fn models_provider(config: &ConfigSummary) -> Option<&str> {
    let mut providers = config
        .model_options
        .iter()
        .map(|option| option.provider.as_str());
    let first = providers.next()?;
    providers.all(|provider| provider == first).then_some(first)
}

/// Models offered to the session as `(name, label)`; hidden ones only when
/// already chosen. Labels are the model names, with the provider only when
/// models of several providers are offered.
pub(in crate::ui) fn models(config: &ConfigSummary) -> Vec<(String, String)> {
    let current = config.model.as_ref().map(|model| &model.name);
    let shared = models_provider(config).is_some();
    config
        .model_options
        .iter()
        .filter(|option| !option.hidden || Some(&option.name) == current)
        .map(|option| {
            let label = if shared {
                option.name.clone()
            } else {
                format!("{}/{}", option.provider, option.name)
            };
            (option.name.clone(), label)
        })
        .collect()
}
