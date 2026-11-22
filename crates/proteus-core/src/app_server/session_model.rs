//! Exact model routing scoped to one durable session.
//! The profile and its shared last-choice preference remain the source baseline.
use anyhow::{Context, Result, ensure};
use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};

use super::{AppServerEvent, AppServerHandle, prepare_assembly};
use crate::{
    core::{AppConfig, ModelConfig, PreparedAssembly, RuntimeRegistry, SessionConfigSnapshot},
    domain::{ModelRef, ReasoningConfig},
};

const FILE: &str = "model-configuration.json";
const MAX_BYTES: u64 = 16 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Configuration {
    binding: String,
    pub model: ModelRef,
    pub reasoning: ReasoningConfig,
    pub stream: bool,
}

pub(super) enum Stored {
    Absent,
    Invalidated,
    Current(Configuration),
}

#[derive(Clone, Copy)]
enum Admission {
    Exact,
    Picker,
}

fn binding(config: &AppConfig, model: &ModelRef) -> Result<String> {
    let route = config
        .module_config
        .get("model")
        .and_then(|routes| routes.get(&model.provider));
    let components = config
        .components
        .iter()
        .filter(|(_, component)| {
            component
                .exports()
                .any(|(slot, id, _)| slot == "model" && id == model.provider)
        })
        .map(|(id, component)| (id, component.execution_identity()))
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&(
        &config.active_provider,
        config.selected_model_config()?,
        route,
        components,
    ))?;
    Ok(digest(&SHA256, &bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub(super) async fn stored(server: &AppServerHandle, baseline: &AppConfig) -> Result<Stored> {
    let Some(store) = server.runtime.session_store() else {
        return Ok(Stored::Absent);
    };
    let selected: Option<Configuration> =
        super::session_selection::read_session(store, FILE).await?;
    match selected {
        None => Ok(Stored::Absent),
        Some(selected) if selected.binding == binding(baseline, &selected.model)? => {
            Ok(Stored::Current(selected))
        }
        Some(_) => Ok(Stored::Invalidated),
    }
}

async fn persist(server: &AppServerHandle, selected: Configuration) -> Result<()> {
    let Some(store) = server.runtime.session_store() else {
        return Ok(());
    };
    let bytes = serde_json::to_vec(&selected)?;
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "session model configuration exceeds its size limit"
    );
    store
        .write_metadata(FILE, serde_json::to_value(selected)?)
        .await
}

pub(super) async fn prepare(
    server: &AppServerHandle,
    baseline: &AppConfig,
    selected: &Configuration,
) -> Result<(AppConfig, PreparedAssembly)> {
    prepare_with_admission(server, baseline, selected, Admission::Exact).await
}

async fn prepare_with_admission(
    server: &AppServerHandle,
    baseline: &AppConfig,
    selected: &Configuration,
    admission: Admission,
) -> Result<(AppConfig, PreparedAssembly)> {
    // The caller may choose only a route configured for this session. Rebinding a
    // different adapter requires a real assembly, never just a changed ModelRef.
    let alias = baseline
        .active_provider
        .as_ref()
        .filter(|alias| {
            baseline
                .providers
                .get(*alias)
                .is_some_and(|profile| profile.provider == selected.model.provider)
        })
        .cloned()
        .or_else(|| {
            baseline
                .providers
                .iter()
                .find(|(_, profile)| profile.provider == selected.model.provider)
                .map(|(alias, _)| alias.clone())
        })
        .context("selected model provider is not configured for this agent")?;
    let mut effective = baseline.clone();
    effective.active_provider = Some(alias.clone());
    let profile = effective
        .providers
        .get_mut(&alias)
        .expect("configured alias");
    profile.model = selected.model.model.clone();
    profile.reasoning = selected.reasoning.clone();
    profile.stream = selected.stream;
    effective.agent_control.model_defaults = Some(ModelConfig {
        provider: selected.model.provider.clone(),
        model: selected.model.model.clone(),
        stream: selected.stream,
        reasoning: selected.reasoning.clone(),
    });
    let assembly = prepare_assembly(&effective, &server.cwd, server.config_path.as_deref()).await?;
    validate(assembly.registry(), baseline, selected, admission).await?;
    Ok((effective, assembly))
}

async fn validate(
    registry: &RuntimeRegistry,
    baseline: &AppConfig,
    selected: &Configuration,
    admission: Admission,
) -> Result<()> {
    ensure!(
        !selected.model.provider.trim().is_empty() && !selected.model.model.trim().is_empty(),
        "model provider and name must not be empty"
    );
    ensure!(
        baseline
            .providers
            .values()
            .any(|profile| profile.provider == selected.model.provider),
        "selected model provider is not configured for this agent"
    );
    if let Some(effort) = &selected.reasoning.effort {
        ensure!(
            !effort.trim().is_empty(),
            "reasoning effort must not be empty"
        );
    }
    let declared_efforts = baseline
        .providers
        .values()
        .filter(|profile| {
            profile.provider == selected.model.provider && profile.model == selected.model.model
        })
        .flat_map(|profile| profile.reasoning_efforts.iter())
        .collect::<std::collections::BTreeSet<_>>();
    match registry.model_catalog().await? {
        Some(catalog) => {
            let entry = catalog
                .models
                .iter()
                .find(|entry| {
                    entry.id == selected.model.model
                        && (!matches!(admission, Admission::Picker) || !entry.hidden)
                })
                .context("selected model is absent from the provider catalog")?;
            if let Some(effort) = &selected.reasoning.effort {
                ensure!(
                    entry.reasoning_efforts.contains(effort),
                    "unsupported reasoning effort {effort:?} for model {:?}",
                    selected.model.model
                );
            }
        }
        None => {
            if matches!(admission, Admission::Picker) {
                ensure!(
                    baseline.providers.values().any(|profile| {
                        profile.provider == selected.model.provider
                            && profile.model == selected.model.model
                    }),
                    "selected model is absent from the configured provider models"
                );
            }
            if let Some(effort) = &selected.reasoning.effort {
                ensure!(
                    declared_efforts.is_empty() || declared_efforts.contains(effort),
                    "unsupported configured reasoning effort {effort:?}"
                );
            }
        }
    }
    if let Some(capabilities) = registry.model_capabilities(&selected.model)? {
        ensure!(
            capabilities.supports_reasoning_config || !selected.reasoning.is_enabled(),
            "selected model does not support reasoning configuration"
        );
    }
    Ok(())
}

impl AppServerHandle {
    /// Exact transient selection for delegated work. Publication and its
    /// session-only metadata have an owner even if the caller disconnects.
    pub async fn apply_model_configuration(
        &self,
        model: ModelRef,
        reasoning: ReasoningConfig,
        stream: bool,
    ) -> Result<()> {
        let server = self.clone();
        tokio::spawn(async move {
            let baseline = server.config.read().await.clone();
            let selected = Configuration {
                binding: binding(&baseline, &model)?,
                model,
                reasoning,
                stream,
            };
            apply(&server, &baseline, selected, true, Admission::Exact).await
        })
        .await
        .context("session model configuration task failed")?
    }
}

/// The picker sends the full model reference so identical IDs can coexist
/// across providers. Keep same-provider changes on the ordinary settings path.
pub(super) async fn select(server: &AppServerHandle, model: ModelRef) -> Result<()> {
    if server
        .runtime
        .model_ref()
        .await
        .is_some_and(|active| active.provider == model.provider)
    {
        return server
            .update_selection(super::session_selection::SelectionUpdate::Model {
                model: model.model,
                expected_provider: Some(model.provider),
            })
            .await;
    }

    let server = server.clone();
    tokio::spawn(async move {
        let baseline = server.config.read().await.clone();
        let mut profiles = baseline
            .providers
            .values()
            .filter(|profile| profile.provider == model.provider);
        let first = profiles
            .next()
            .context("selected model provider is not configured for this agent")?;
        let exact = std::iter::once(first)
            .chain(profiles)
            .find(|profile| profile.model == model.model);
        let selected = Configuration {
            binding: binding(&baseline, &model)?,
            reasoning: exact
                .map(|profile| profile.reasoning.clone())
                .unwrap_or_default(),
            stream: exact.unwrap_or(first).stream,
            model,
        };
        apply(&server, &baseline, selected, true, Admission::Picker).await
    })
    .await
    .context("session model selection task failed")?
}

async fn apply(
    server: &AppServerHandle,
    baseline: &AppConfig,
    selected: Configuration,
    write: bool,
    admission: Admission,
) -> Result<()> {
    let runs = server.runs.lock().await;
    runs.ensure_idle_for_model_configuration()?;
    let lock = server
        .config_path
        .as_deref()
        .map(super::config_builder::path_lock)
        .transpose()?;
    let _guard = match lock {
        Some(lock) => Some(lock.lock_owned().await),
        None => None,
    };
    ensure!(
        serde_json::to_value(&*server.config.read().await)? == serde_json::to_value(baseline)?,
        "profile changed during model configuration; retry selection"
    );
    let _transition = server.settings_transition.lock().await;
    let unchanged = selected.clone();
    if server
        .runtime
        .apply_bound_model_configuration(
            &selected.model,
            &selected.reasoning,
            selected.stream,
            |snapshot| async move {
                validate(&snapshot.registry, baseline, &unchanged, admission).await?;
                if write {
                    persist(server, unchanged).await?;
                }
                Ok(())
            },
        )
        .await?
    {
        return Ok(());
    }
    let behavior = super::session_behavior::effective(server, baseline).await;
    let (effective, assembly) =
        prepare_with_admission(server, &behavior, &selected, admission).await?;
    let snapshot = SessionConfigSnapshot::from_runtime_config(
        &effective,
        assembly.registry(),
        server.permission_mode().await,
        server.runtime.approval_available(),
    );
    let model = selected.model.clone();
    let reasoning = selected.reasoning.clone();
    let report = server
        .runtime
        .reload_assembly_with_commit(
            assembly,
            Some(snapshot),
            Some(model),
            None,
            Some(reasoning),
            || async {
                if write {
                    persist(server, selected).await?;
                }
                Ok(())
            },
        )
        .await?;
    let _ = server.events.send(AppServerEvent::ModulesReloaded {
        old_epoch: report.old_epoch,
        new_epoch: report.new_epoch,
        tool_names: report.tool_names,
    });
    Ok(())
}

pub(super) async fn restore(server: &AppServerHandle, fresh: bool) -> Result<()> {
    if !fresh {
        let baseline = server.config.read().await.clone();
        if let Stored::Current(selected) = stored(server, &baseline).await? {
            apply(server, &baseline, selected, false, Admission::Exact).await?;
        }
    }
    Ok(())
}

/// Human changes in a program-scoped child remain local to that session.
pub(super) async fn persist_if_scoped(
    server: &AppServerHandle,
    effective: &AppConfig,
    model: &ModelRef,
    reasoning: &ReasoningConfig,
) -> Result<bool> {
    let baseline = server.config.read().await.clone();
    if let Stored::Current(_) = stored(server, &baseline).await? {
        persist(
            server,
            Configuration {
                binding: binding(&baseline, model)?,
                model: model.clone(),
                reasoning: reasoning.clone(),
                stream: effective.active_model_config()?.stream,
            },
        )
        .await?;
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests;
