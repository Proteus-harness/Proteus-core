//! One fresh child gets one exact selection from the admitted parent turn.
//! Resume belongs to the existing session and never reapplies a new parent model.
use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::{
    contracts::{AgentControlRequest, AgentWorkflowContext},
    core::{ModelConfig, ProviderProfileConfig, config_selection},
};

use super::config::ProcessRoleConfig;

pub(super) struct ModelChoices {
    pub presets: BTreeMap<String, ProviderProfileConfig>,
    pub defaults: Option<ModelConfig>,
}

fn optional_string<'a>(metadata: &'a Value, key: &str) -> Result<Option<&'a str>> {
    metadata
        .get(key)
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .with_context(|| format!("subagent {key} override must be a nonempty string"))
        })
        .transpose()
}

impl ModelChoices {
    pub(super) fn resolve(
        &self,
        role: &ProcessRoleConfig,
        request: &AgentControlRequest,
        ctx: &AgentWorkflowContext,
    ) -> Result<Option<ModelConfig>> {
        let requested = optional_string(&request.metadata, "model")?;
        let effort = optional_string(&request.metadata, "effort")?;
        if request
            .metadata
            .get("task_id")
            .and_then(Value::as_str)
            .is_some()
        {
            if requested.is_some() || effort.is_some() {
                bail!("cannot override model or effort while resuming an existing subagent");
            }
            return Ok(None);
        }

        if let Some(profile) = requested.and_then(|name| self.presets.get(name)) {
            let mut selected = profile.to_model_config()?;
            if let Some(effort) = effort {
                selected.reasoning.effort = Some(effort.to_owned());
            }
            return Ok(Some(selected));
        }

        // External references can be interpreted by a custom child executable.
        // Preserve that boundary: absence of a canonical role choice means the
        // external child owns its default, rather than reading it in this host.
        if role.model.is_none() && config_selection(Path::new(&role.config))?.is_empty() {
            if requested.is_none() {
                if effort.is_some() {
                    bail!("effort override for an external child requires an explicit model");
                }
                return Ok(None);
            }
        }
        let configured = role.model.clone();
        let mut selected = configured.or_else(|| {
            ctx.model_ref.as_ref().map(|model| ModelConfig {
                provider: model.provider.clone(),
                model: model.model.clone(),
                stream: self.defaults.as_ref().is_none_or(|model| model.stream),
                reasoning: ctx.reasoning.clone(),
            })
        });
        if requested.is_some() || effort.is_some() {
            let model = selected
                .as_mut()
                .context("subagent model override requires a configured model route")?;
            if let Some(requested) = requested {
                model.model = requested.to_owned();
            }
            if let Some(effort) = effort {
                model.reasoning.effort = Some(effort.to_owned());
            }
        }
        Ok(selected)
    }
}
