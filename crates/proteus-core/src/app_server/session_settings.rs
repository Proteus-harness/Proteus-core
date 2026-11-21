//! One settings projection and validator shared by HTTP, extensions and ACP.

use anyhow::{Result, bail};
use proteus_contracts::app_protocol::settings::{
    SessionSetting, SessionSettingChoice, SessionSettingControl, SessionSettingUpdate,
};

use super::model_selection::{ModelSelection, SelectionSummary};
use crate::domain::{ModelRef, PermissionMode, ReasoningConfig};

const DEFAULT_EFFORT: &str = "_default";

#[derive(Debug)]
pub(super) struct InvalidSessionSetting(String);

impl std::fmt::Display for InvalidSessionSetting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for InvalidSessionSetting {}

fn choice(
    value: impl Into<String>,
    name: impl Into<String>,
    description: Option<String>,
) -> SessionSettingChoice {
    SessionSettingChoice {
        value: value.into(),
        name: name.into(),
        description,
    }
}

fn select(
    id: &str,
    name: &str,
    category: &str,
    value: String,
    options: Vec<SessionSettingChoice>,
) -> SessionSetting {
    SessionSetting {
        id: id.into(),
        name: name.into(),
        category: Some(category.into()),
        control: SessionSettingControl::Select { value, options },
    }
}

pub(super) fn render(
    mode: PermissionMode,
    active: &Option<ModelRef>,
    reasoning: &ReasoningConfig,
    summary: &SelectionSummary,
) -> Vec<SessionSetting> {
    let mut settings = vec![select(
        "mode",
        "Permissions",
        "mode",
        mode.as_str().to_owned(),
        PermissionMode::SELECTABLE.into_iter().map(|mode| match mode {
            PermissionMode::Plan => choice(mode.as_str(), "Только чтение",
                Some("Доступны только инструменты, объявленные как чтение. Остальные вызовы запрещены.".into())),
            PermissionMode::Normal => choice(mode.as_str(), "С подтверждением",
                Some("Подтверждения и запреты определяются правилами выбранного профиля.".into())),
            PermissionMode::FullAccess => choice(mode.as_str(), "Полный доступ",
                Some("Вызовы не требуют подтверждения. Явные запреты правил и ограничения среды сохраняются.".into())),
            PermissionMode::Auto => unreachable!("legacy mode is not selectable"),
        }).collect(),
    )];
    // Preserve the active legacy value without advertising it as a new choice.
    if mode == PermissionMode::Auto
        && let SessionSettingControl::Select { options, .. } = &mut settings[0].control
    {
        options.push(choice(
            "auto",
            "Автоматически (прежний режим)",
            Some("Разрешены инструменты чтения и записи; команды, сеть и опасные инструменты запрещены.".into()),
        ));
    }
    if summary.error.is_some() {
        return settings;
    }
    let models: Vec<_> = summary
        .models
        .iter()
        .filter(|model| {
            !model.hidden || Some(model.name.as_str()) == active.as_ref().map(|m| m.model.as_str())
        })
        .map(|model| choice(&model.name, &model.label, model.description.clone()))
        .collect();
    if !models.is_empty() {
        settings.push(select(
            "model",
            "Model",
            "model",
            active.as_ref().map(|m| m.model.clone()).unwrap_or_default(),
            models,
        ));
    }
    if !summary.efforts.is_empty() {
        let mut efforts = vec![choice(
            DEFAULT_EFFORT,
            "Default",
            Some("Let the provider choose the reasoning effort".into()),
        )];
        efforts.extend(
            summary
                .efforts
                .iter()
                .map(|effort| choice(format!("effort:{effort}"), effort, None)),
        );
        settings.push(select(
            "reasoning_effort",
            "Reasoning",
            "thought_level",
            reasoning
                .effort
                .as_deref()
                .map(|effort| format!("effort:{effort}"))
                .unwrap_or_else(|| DEFAULT_EFFORT.into()),
            efforts,
        ));
    }
    settings
}

/// Compose every session setting from an already resolved model selection.
/// HTTP's local projection must not start a second catalog discovery.
pub(super) async fn compose(
    server: &super::AppServerHandle,
    mode: PermissionMode,
    active: &Option<ModelRef>,
    reasoning: &ReasoningConfig,
    summary: &SelectionSummary,
) -> Vec<SessionSetting> {
    let mut settings = render(mode, active, reasoning, summary);
    if let Some(behavior) = super::session_behavior::setting(server).await {
        settings.insert(0, behavior);
    }
    settings
}

impl super::AppServerHandle {
    pub async fn session_settings(&self) -> Vec<SessionSetting> {
        let ModelSelection {
            active,
            reasoning,
            summary,
        } = self.model_selection().await;
        compose(
            self,
            self.permission_mode().await,
            &active,
            &reasoning,
            &summary,
        )
        .await
    }

    pub async fn set_session_setting(&self, update: SessionSettingUpdate) -> Result<()> {
        let settings = self.session_settings().await;
        let setting = settings
            .iter()
            .find(|setting| setting.id == update.id)
            .ok_or_else(|| {
                InvalidSessionSetting(format!(
                    "unknown or unavailable session setting {:?}",
                    update.id
                ))
            })?;
        let SessionSettingControl::Select { options, .. } = &setting.control else {
            return Err(InvalidSessionSetting("unsupported session setting control".into()).into());
        };
        let serde_json::Value::String(value) = update.value else {
            return Err(
                InvalidSessionSetting("select settings require a string value".into()).into(),
            );
        };
        // Older clients may still submit their cached legacy mode choice.
        let legacy_mode = update.id == "mode" && value == PermissionMode::Auto.as_str();
        if !legacy_mode && !options.iter().any(|option| option.value == value) {
            return Err(InvalidSessionSetting(format!(
                "value is absent from session setting {:?}",
                update.id
            ))
            .into());
        }
        match update.id.as_str() {
            "mode" => {
                self.set_permission_mode(serde_json::from_value(serde_json::Value::String(value))?)
                    .await?
            }
            "behavior_mode" => self.set_behavior_mode(value).await?,
            "model" => self.set_model_name(value).await?,
            "reasoning_effort" => {
                self.set_reasoning_effort(value.strip_prefix("effort:").map(str::to_owned))
                    .await?
            }
            _ => bail!("unsupported session setting {:?}", update.id),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_choices_keep_full_access_canonical_and_legacy_auto_distinct() {
        let summary = SelectionSummary {
            models: vec![],
            efforts: vec![],
            error: None,
        };
        for mode in PermissionMode::SELECTABLE
            .into_iter()
            .chain([PermissionMode::Auto])
        {
            let settings = render(mode, &None, &ReasoningConfig::default(), &summary);
            let SessionSettingControl::Select { value, options } = &settings[0].control else {
                panic!("mode select");
            };
            assert_eq!(value, mode.as_str());
            let values: Vec<_> = options.iter().map(|option| option.value.as_str()).collect();
            assert_eq!(&values[..3], ["plan", "normal", "full_access"]);
            assert_eq!(values.contains(&"auto"), mode == PermissionMode::Auto);
        }
    }
}
