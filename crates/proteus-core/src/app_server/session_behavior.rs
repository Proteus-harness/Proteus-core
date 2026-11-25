//! Per-chat behavior selection. Only an ID and explicit permission override are durable;
//! the current profile remains the authority for every overlay definition.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::{AppServerEvent, AppServerHandle, prepare_assembly};
use crate::{
    core::{AppConfig, InstructionSourceConfig, PreparedAssembly, SessionConfigSnapshot},
    domain::PermissionMode,
    model_standard::InstructionKind,
};

const FILE: &str = "behavior-selection.json";
const MAX_BYTES: u64 = 16 * 1024;
pub(super) const DEFAULT: &str = "_default";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Selection {
    pub(super) selected: Option<String>,
    pub(super) permission_override: Option<PermissionMode>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    binding: String,
    selection: Selection,
}

fn binding(server: &AppServerHandle) -> Result<String> {
    Ok(serde_json::to_string(&(
        server
            .config_path
            .as_deref()
            .map(super::session_selection::resolved_path)
            .transpose()?,
        &server.cwd,
    ))?)
}

pub(super) fn resolved(mut selection: Selection, config: &AppConfig) -> Selection {
    if selection
        .selected
        .as_ref()
        .is_some_and(|id| !config.modes.contains_key(id))
    {
        selection = Selection::default();
    }
    selection
}

pub(super) fn overlay(config: &AppConfig, selection: &Selection) -> AppConfig {
    let mut effective = config.clone();
    if let Some(definition) = selection
        .selected
        .as_ref()
        .and_then(|id| config.modes.get(id))
    {
        if let Some(tools) = &definition.tools {
            effective.tools.enabled = tools.clone();
            effective.behavior_tools = Some(tools.clone());
        }
        if let Some(hooks) = &definition.hooks {
            effective.modules.hooks = hooks.clone();
        }
        if let Some(mode) = definition.permission_mode {
            effective.permissions.mode = mode;
        }
        if let Some(text) = &definition.instructions {
            effective.instructions.push(InstructionSourceConfig {
                kind: InstructionKind::UserPreference,
                text: Some(text.clone()),
                file: None,
                priority: 100,
            });
        }
    }
    if let Some(mode) = selection.permission_override {
        effective.permissions.mode = mode;
    }
    effective
}

pub(super) async fn effective(server: &AppServerHandle, baseline: &AppConfig) -> AppConfig {
    overlay(
        baseline,
        &resolved(server.behavior.read().await.clone(), baseline),
    )
}

pub(super) async fn prepare(
    server: &AppServerHandle,
    config: &Arc<AppConfig>,
    selected: &Selection,
) -> Result<PreparedAssembly> {
    let effective = if *selected == Selection::default() {
        config.clone()
    } else {
        Arc::new(overlay(config, selected))
    };
    effective.validate_semantics()?;
    match super::session_model::stored(server, config).await? {
        super::session_model::Stored::Current(model) => {
            super::session_model::prepare(server, &effective, &model).await
        }
        _ => prepare_assembly(effective, &server.cwd, server.config_path.as_deref()).await,
    }
}

fn selection_bytes(server: &AppServerHandle, selected: &Selection) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(&Stored {
        binding: binding(server)?,
        selection: selected.clone(),
    })?;
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "behavior selection exceeds its size limit"
    );
    Ok(bytes)
}

async fn persist(server: &AppServerHandle, selected: &Selection) -> Result<()> {
    let Some(store) = server.runtime.session_store() else {
        return Ok(());
    };
    let bytes = selection_bytes(server, selected)?;
    store
        .write_metadata(FILE, serde_json::from_slice(&bytes)?)
        .await
}

/// Metadata is cleared before the source commit so a crash can only conservatively
/// lose a selection. Ordinary source failures restore exact previous metadata.
pub(super) async fn commit_profile<F, Fut>(
    server: &AppServerHandle,
    invalidate: bool,
    commit: F,
) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let Some(store) = server.runtime.session_store().filter(|_| invalidate) else {
        return Box::pin(commit()).await;
    };
    let bytes = selection_bytes(server, &Selection::default())?;
    let previous = store.read_metadata_document(FILE).await?;
    let document = store
        .compare_exchange_metadata(
            FILE.to_owned(),
            previous.as_ref().map(|document| document.etag.clone()),
            Some(serde_json::from_slice(&bytes)?),
        )
        .await?
        .context("behavior selection write did not return a document")?;
    if let Err(error) = Box::pin(commit()).await {
        let rollback = store
            .compare_exchange_metadata(
                FILE.to_owned(),
                Some(document.etag),
                previous.map(|document| document.value),
            )
            .await;
        return match rollback {
            Ok(_) => Err(error),
            Err(rollback) => Err(error.context(format!(
                "behavior selection rollback also failed: {rollback:#}"
            ))),
        };
    }
    Ok(())
}

async fn apply(server: &AppServerHandle, selected: Selection, write: bool) -> Result<()> {
    let runs = server.runs.lock().await;
    runs.ensure_idle_for_model_configuration()?;
    let path_lock = server
        .config_path
        .as_deref()
        .map(super::config_builder::path_lock)
        .transpose()?;
    let _path_guard = match path_lock {
        Some(lock) => Some(lock.lock_owned().await),
        None => None,
    };
    let _transition = server.settings_transition.lock().await;
    let baseline = server.config.read().await.clone();
    if write {
        ensure!(
            selected
                .selected
                .as_ref()
                .is_none_or(|id| baseline.modes.contains_key(id)),
            "behavior mode was removed; refresh settings"
        );
    }
    let selected = resolved(selected, &baseline);
    let assembly = prepare(server, &baseline, &selected).await?;
    let effective = assembly.plan().config();
    let mode = effective.permissions.mode;
    let (_, model, reasoning, _) = server.runtime.configuration_view().await;
    let snapshot = SessionConfigSnapshot::from_runtime_config(
        effective,
        assembly.registry(),
        mode,
        server.runtime.approval_available(),
    );
    let report = server
        .runtime
        .reload_assembly_with_commit(
            assembly,
            Some(snapshot),
            model,
            Some(mode),
            Some(reasoning),
            || async {
                if write {
                    persist(server, &selected).await?;
                }
                Ok(())
            },
        )
        .await?;
    *server.behavior.write().await = selected;
    let _ = server.events.send(AppServerEvent::ModulesReloaded {
        old_epoch: report.old_epoch,
        new_epoch: report.new_epoch,
        tool_names: report.tool_names,
    });
    Ok(())
}

impl AppServerHandle {
    pub(super) async fn set_behavior_mode(&self, id: String) -> Result<()> {
        let server = self.clone();
        tokio::spawn(async move {
            let baseline = server.config.read().await;
            ensure!(
                id == DEFAULT || baseline.modes.contains_key(&id),
                "unknown behavior mode {id:?}"
            );
            drop(baseline);
            apply(
                &server,
                Selection {
                    selected: (id != DEFAULT).then_some(id),
                    permission_override: None,
                },
                true,
            )
            .await
        })
        .await
        .context("behavior selection task failed")?
    }

    pub(super) async fn set_behavior_permission(&self, mode: PermissionMode) -> Result<()> {
        let server = self.clone();
        tokio::spawn(async move {
            let _transition = server.settings_transition.lock().await;
            let baseline = server.config.read().await.clone();
            let mut selected = resolved(server.behavior.read().await.clone(), &baseline);
            let durable = !baseline.modes.is_empty() || selected != Selection::default();
            if durable {
                selected.permission_override = Some(mode);
            }
            server
                .runtime
                .set_permission_mode_with_commit(mode, || async {
                    if durable {
                        persist(&server, &selected).await?;
                    }
                    Ok(())
                })
                .await?;
            if durable {
                *server.behavior.write().await = selected;
            }
            Ok(())
        })
        .await
        .context("permission selection task failed")?
    }
}

pub(super) async fn restore(server: &AppServerHandle, fresh: bool) -> Result<()> {
    if fresh {
        return Ok(());
    }
    let Some(store) = server.runtime.session_store() else {
        return Ok(());
    };
    let stored: Option<Stored> = super::session_selection::read_session(store, FILE).await?;
    if let Some(stored) = stored {
        let baseline = server.config.read().await.clone();
        if stored.binding == binding(server)? {
            let invalidated = stored
                .selection
                .selected
                .as_ref()
                .is_some_and(|id| !baseline.modes.contains_key(id));
            let selected = resolved(stored.selection, &baseline);
            if invalidated {
                // The source may have lost this ID while this process was closed.
                // Make cold invalidation durable before a later undo can recreate it.
                persist(server, &selected).await?;
            }
            if selected != Selection::default() {
                apply(server, selected, false).await?;
            }
        }
    }
    Ok(())
}

pub(super) async fn setting(
    server: &AppServerHandle,
) -> Option<proteus_contracts::app_protocol::settings::SessionSetting> {
    use proteus_contracts::app_protocol::settings::{
        SessionSetting, SessionSettingChoice, SessionSettingControl,
    };
    let baseline = server.config.read().await;
    if baseline.modes.is_empty() {
        return None;
    }
    let selection = resolved(server.behavior.read().await.clone(), &baseline);
    let mut options = vec![SessionSettingChoice {
        value: DEFAULT.into(),
        name: "Из конфига".into(),
        description: None,
    }];
    options.extend(
        baseline
            .modes
            .iter()
            .map(|(id, definition)| SessionSettingChoice {
                value: id.clone(),
                name: definition.title.clone().unwrap_or_else(|| id.clone()),
                description: None,
            }),
    );
    Some(SessionSetting {
        id: "behavior_mode".into(),
        name: "Режим поведения".into(),
        category: Some("behavior".into()),
        control: SessionSettingControl::Select {
            value: selection.selected.unwrap_or_else(|| DEFAULT.into()),
            options,
        },
    })
}

#[cfg(test)]
mod tests;
