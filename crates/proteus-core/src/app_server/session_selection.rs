//! Small user preferences, separate from canonical history and shared profiles.
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};

use crate::{
    core::{AgentRuntime, AppConfig, config_store_root},
    domain::{ModelRef, ReasoningConfig},
};

const FILE: &str = "model-selection.json";
const MAX_BYTES: u64 = 16 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    version: u32,
    binding: String,
    model: ModelRef,
    reasoning: ReasoningConfig,
}

fn sha256(bytes: &[u8]) -> String {
    digest(&SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn resolved_path(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    absolute
        .ancestors()
        .find_map(|ancestor| {
            std::fs::canonicalize(ancestor)
                .ok()
                .map(|root| root.join(absolute.strip_prefix(ancestor).expect("ancestor")))
        })
        .context("cannot resolve selection profile path")
}

fn binding(config: &AppConfig, path: &Path) -> Result<Option<String>> {
    let Some(model) = config.selected_model_config()? else {
        return Ok(None);
    };
    // Hash route configuration rather than storing credentials or guessing
    // provider-owned endpoint keys. An explicitly changed profile baseline or
    // adapter/route invalidates an older choice instead of undoing a reload.
    let route = config
        .module_config
        .get("model")
        .and_then(|models| models.get(&model.provider));
    let components: Vec<_> = config
        .components
        .iter()
        .filter(|(_, component)| {
            component
                .exports()
                .any(|(slot, id, _)| slot == "model" && id == model.provider)
        })
        .collect();
    let bytes = serde_json::to_vec(&(
        resolved_path(path)?,
        &config.profile.name,
        &config.active_provider,
        &model,
        route,
        components,
    ))?;
    Ok(Some(sha256(&bytes)))
}

fn last_path(config: &AppConfig, path: &Path) -> Result<PathBuf> {
    // Exactly one value file per profile/provider alias. Route revisions affect
    // the binding stored inside, never create a growing file for each edit.
    let key = serde_json::to_vec(&(
        resolved_path(path)?,
        &config.profile.name,
        &config.active_provider,
    ))?;
    Ok(config_store_root(path)
        .join("model-selections")
        .join(format!("{}.json", sha256(&key))))
}

fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("read selection {}", path.display()));
        }
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "selection file exceeds {MAX_BYTES} bytes: {}",
        path.display()
    );
    Ok(Some(bytes))
}

fn read_selection(path: &Path, expected: &str) -> Result<Option<Selection>> {
    let Some(bytes) = read_bytes(path)? else {
        return Ok(None);
    };
    let selection: Selection = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid selection {}", path.display()))?;
    ensure!(
        selection.version == 1,
        "unsupported selection version {}",
        selection.version
    );
    ensure!(
        !selection.model.model.trim().is_empty(),
        "saved model is empty"
    );
    Ok((selection.binding == expected).then_some(selection))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("selection path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".selection-{}.tmp", crate::domain::new_call_id()));
    let result = (|| -> Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        if let Ok(metadata) = std::fs::metadata(path) {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.with_context(|| format!("persist selection {}", path.display()))
}

fn persist_pair(session: &Path, last: &Path, selection: &Selection) -> Result<()> {
    let bytes = serde_json::to_vec(selection)?;
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "selection exceeds {MAX_BYTES} bytes"
    );
    let previous = read_bytes(session)?;
    atomic_write(session, &bytes)?;
    // Each file is atomic, not a cross-file crash transaction. A crash between
    // these renames can leave the session newer than the default for NEW chats.
    // Ordinary failures roll back the session; a shared path lock prevents a
    // second session from racing that rollback and the global last-used write.
    if let Err(error) = atomic_write(last, &bytes) {
        let rollback = match previous {
            Some(previous) => atomic_write(session, &previous),
            None => std::fs::remove_file(session).map_err(Into::into),
        };
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback) => Err(error.context(format!(
                "session selection rollback also failed: {rollback:#}"
            ))),
        };
    }
    Ok(())
}

pub(super) enum SelectionUpdate {
    Model(String),
    ReasoningEnabled(bool),
    ReasoningEffort(Option<String>),
}

impl super::AppServerHandle {
    /// Once admitted, persistence and runtime publication have one owner even
    /// when the request disconnects or its calling future is cancelled.
    pub(super) async fn update_selection(&self, update: SelectionUpdate) -> Result<()> {
        let server = self.clone();
        tokio::spawn(async move {
            match update {
                SelectionUpdate::Model(model) => {
                    server
                        .runtime
                        .set_model_name_with_commit(model, |config, model, reasoning| {
                            server.persist_selection(config, model, reasoning)
                        })
                        .await
                }
                SelectionUpdate::ReasoningEnabled(enabled) => {
                    server
                        .runtime
                        .set_reasoning_enabled_with_commit(enabled, |config, model, reasoning| {
                            server.persist_selection(config, model, reasoning)
                        })
                        .await
                }
                SelectionUpdate::ReasoningEffort(effort) => {
                    server
                        .runtime
                        .set_reasoning_effort_with_commit(effort, |config, model, reasoning| {
                            server.persist_selection(config, model, reasoning)
                        })
                        .await
                }
            }
        })
        .await
        .context("selection update task failed")?
    }

    pub(super) async fn persist_selection(
        &self,
        config: AppConfig,
        model: ModelRef,
        reasoning: ReasoningConfig,
    ) -> Result<()> {
        let (Some(path), Some(store)) = (&self.config_path, self.runtime.session_store()) else {
            return Ok(());
        };
        let binding = binding(&config, path)?.context("selection has no active provider")?;
        let configured = config.active_model_config()?;
        ensure!(
            model.provider == configured.provider,
            "selection provider does not match published profile"
        );
        let selection = Selection {
            version: 1,
            binding: binding.clone(),
            model,
            reasoning,
        };
        let last = last_path(&config, path)?;
        let lock = super::config_builder::path_lock(&last)?;
        let guard = lock.lock_owned().await;
        store
            .with_metadata_transaction(move |directory| {
                // The lock belongs to the actual writer, not its awaiter. Dropping
                // a JoinHandle never unlocks an in-flight write/rollback.
                let _guard = guard;
                let session = directory.join(FILE);
                #[cfg(test)]
                tests::pause_persistence(&session);
                persist_pair(&session, &last, &selection)
            })
            .await
    }
}

pub(super) async fn restore(
    runtime: &Arc<AgentRuntime>,
    config: &AppConfig,
    config_path: Option<&Path>,
    is_new: bool,
) -> Result<()> {
    let (Some(path), Some(store)) = (config_path, runtime.session_store()) else {
        return Ok(());
    };
    let Some(binding) = binding(config, path)? else {
        return Ok(());
    };
    let session = store.session_dir().join(FILE);
    let source = if is_new {
        last_path(config, path)?
    } else {
        session.clone()
    };
    let selected = tokio::task::spawn_blocking(move || read_selection(&source, &binding))
        .await
        .context("selection restore worker failed")??;
    if let Some(selection) = selected {
        let configured = config.active_model_config()?;
        ensure!(
            configured.provider == selection.model.provider,
            "saved selection provider mismatch"
        );
        runtime
            .restore_model_selection(selection.model.clone(), selection.reasoning.clone())
            .await?;
        if is_new {
            // Freeze the inherited choice for this session. Subsequent changes
            // in another chat must never override it on resume.
            store
                .with_metadata_transaction(move |directory| {
                    atomic_write(&directory.join(FILE), &serde_json::to_vec(&selection)?)
                })
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
