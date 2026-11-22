pub(super) use proteus_contracts::app_protocol::config::ModelOption;

use futures_util::{StreamExt, stream};
use std::{collections::BTreeMap, time::Duration};
use tokio::time::{Instant, timeout_at};

use super::config_summary::{configured_model_options, configured_reasoning_effort_options};
use crate::{
    contracts::ModelCatalog,
    core::{AppConfig, RuntimeRegistry},
    domain::{ModelRef, ReasoningConfig},
};

pub(super) struct SelectionSummary {
    pub models: Vec<ModelOption>,
    pub efforts: Vec<String>,
    pub error: Option<String>,
}

pub(super) struct ModelSelection {
    pub active: Option<ModelRef>,
    pub reasoning: ReasoningConfig,
    pub summary: SelectionSummary,
}

const INACTIVE_CATALOG_LIMIT: Duration = Duration::from_secs(6);
const INACTIVE_CATALOG_CONCURRENCY: usize = 4;

impl super::AppServerHandle {
    pub(super) async fn model_selection(&self) -> ModelSelection {
        let active = self.runtime.model_ref().await;
        let reasoning = self.runtime.reasoning().await;
        let config = self.config.read().await.clone();
        let summary = selection_summary(
            &config,
            &active,
            &reasoning,
            self.runtime.model_catalog().await,
        );
        ModelSelection {
            active,
            reasoning,
            summary,
        }
    }
}

fn discovered_options(provider: &str, catalog: ModelCatalog) -> impl Iterator<Item = ModelOption> {
    catalog.models.into_iter().map(move |model| ModelOption {
        provider: provider.to_owned(),
        name: model.id,
        label: model.display_name,
        description: model.description,
        hidden: model.hidden,
        reasoning_efforts: model.reasoning_efforts,
        default_reasoning_effort: model.default_reasoning_effort,
    })
}

fn configured_option(model: ModelRef) -> ModelOption {
    ModelOption {
        label: model.model.clone(),
        provider: model.provider,
        name: model.model,
        ..Default::default()
    }
}

/// Catalogs are grouped by the configured model route, not by model names:
/// different providers may offer the same model ID. Keep provider order stable
/// even when their independent metadata calls complete in a different order.
pub(super) async fn inactive_model_options(
    config: &AppConfig,
    active: Option<&ModelRef>,
    registry: &RuntimeRegistry,
    discover: bool,
) -> Vec<ModelOption> {
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for model in configured_model_options(config) {
        if !active.is_some_and(|active| active.provider == model.provider) {
            groups
                .entry(model.provider.clone())
                .or_default()
                .push(configured_option(model));
        }
    }
    let deadline = Instant::now() + INACTIVE_CATALOG_LIMIT;
    stream::iter(groups.into_iter().map(|(provider, configured)| async move {
        if !discover {
            return configured;
        }
        if Instant::now() >= deadline {
            return Vec::new();
        }
        match timeout_at(deadline, registry.inactive_model_catalog(&provider)).await {
            Ok(Ok(Some(catalog))) => discovered_options(&provider, catalog).collect(),
            Ok(Ok(None)) => configured,
            _ => Vec::new(),
        }
    }))
    .buffered(INACTIVE_CATALOG_CONCURRENCY)
    .flat_map(stream::iter)
    .collect()
    .await
}

pub(super) fn selection_summary(
    config: &AppConfig,
    active: &Option<ModelRef>,
    reasoning: &ReasoningConfig,
    catalog: anyhow::Result<Option<ModelCatalog>>,
) -> SelectionSummary {
    let Some(active) = active else {
        return SelectionSummary {
            models: Vec::new(),
            efforts: Vec::new(),
            error: None,
        };
    };
    match catalog {
        Ok(Some(catalog)) => SelectionSummary {
            efforts: catalog
                .models
                .iter()
                .find(|model| model.id == active.model)
                .map(|model| model.reasoning_efforts.clone())
                .unwrap_or_default(),
            models: discovered_options(&active.provider, catalog).collect(),
            error: None,
        },
        Ok(None) => {
            let mut efforts = configured_reasoning_effort_options(config, active, reasoning);
            if !efforts.iter().any(|value| value == "none") {
                efforts.insert(0, "none".into());
            }
            let mut configured = configured_model_options(config);
            if !configured.contains(active) {
                configured.insert(0, active.clone());
            }
            SelectionSummary {
                models: configured
                    .into_iter()
                    .filter(|model| model.provider == active.provider)
                    .map(configured_option)
                    .collect(),
                efforts,
                error: None,
            }
        }
        Err(error) => SelectionSummary {
            models: Vec::new(),
            efforts: Vec::new(),
            error: Some(format!("{error:#}")),
        },
    }
}
