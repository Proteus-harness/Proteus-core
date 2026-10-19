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
        format!("{mode:?}").to_lowercase(),
        vec![
            choice(
                "normal",
                "Normal",
                Some("Use the configured approval policy".into()),
            ),
            choice(
                "auto",
                "Auto",
                Some("Allow actions according to the active policy".into()),
            ),
            choice(
                "plan",
                "Plan",
                Some("Read and plan without making changes".into()),
            ),
        ],
    )];
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

impl super::AppServerHandle {
    pub async fn session_settings(&self) -> Vec<SessionSetting> {
        let ModelSelection {
            active,
            reasoning,
            summary,
        } = self.model_selection().await;
        render(self.permission_mode().await, &active, &reasoning, &summary)
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
        if !options.iter().any(|option| option.value == value) {
            return Err(InvalidSessionSetting(format!(
                "value is absent from session setting {:?}",
                update.id
            ))
            .into());
        }
        match update.id.as_str() {
            "mode" => {
                self.set_permission_mode(serde_json::from_value(serde_json::Value::String(value))?)
                    .await
            }
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
