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
    encode_digest(digest(&SHA256, bytes).as_ref())
}

fn encode_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

pub(super) fn resolved_path(path: &Path) -> Result<PathBuf> {
    let selection = crate::core::config_selection(path)?;
    let absolute = std::path::absolute(crate::core::physical_config_path(path))?;
    let mut resolved = absolute
        .ancestors()
        .find_map(|ancestor| {
            std::fs::canonicalize(ancestor)
                .ok()
                .map(|root| root.join(absolute.strip_prefix(ancestor).expect("ancestor")))
        })
        .context("cannot resolve selection profile path")?;
    for role in selection {
        resolved = crate::core::config_reference(&resolved, &role)?;
    }
    Ok(resolved)
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
        .map(|(id, component)| (id, component.execution_identity()))
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

/// Hot publication follows the same invalidation as cold preference restore.
/// Delegated session configurations have a separate binding and policy.
pub(super) fn binding_changed(previous: &AppConfig, next: &AppConfig, path: &Path) -> Result<bool> {
    Ok(binding(previous, path)? != binding(next, path)?)
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

pub(super) fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
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

pub(super) async fn read_session<T: serde::de::DeserializeOwned>(
    store: &crate::core::SessionStore,
    key: &str,
) -> Result<Option<T>> {
    let Some(document) = store.read_metadata_document(key).await? else {
        return Ok(None);
    };
    // Session preferences retain their small application-level limit even if
    // the selected storage supports larger generic metadata documents.
    struct Bound(usize);
    impl Write for Bound {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_BYTES as usize - self.0 {
                return Err(std::io::Error::other("selection exceeds its size limit"));
            }
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Bound(0), &document.value)?;
    Ok(Some(serde_json::from_value(document.value)?))
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

pub(super) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
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

async fn persist_pair(
    store: &crate::core::SessionStore,
    last: PathBuf,
    selection: Selection,
) -> Result<()> {
    let bytes = serde_json::to_vec(&selection)?;
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "selection exceeds {MAX_BYTES} bytes"
    );
    let previous = store.read_metadata_document(FILE).await?;
    let written = store
        .compare_exchange_metadata(
            FILE.to_owned(),
            previous.as_ref().map(|document| document.etag.clone()),
            Some(serde_json::to_value(&selection)?),
        )
        .await?
        .context("session selection write did not return a document")?;
    // Global last-used preferences are app-owned. On ordinary failure restore
    // the exact session value only if our own write still owns its version.
    if let Err(error) = tokio::task::spawn_blocking(move || atomic_write(&last, &bytes))
        .await
        .context("last-used selection writer failed")
        .and_then(|result| result)
    {
        let rollback = store
            .compare_exchange_metadata(
                FILE.to_owned(),
                Some(written.etag),
                previous.map(|document| document.value),
            )
            .await;
        return match rollback {
            Ok(_) => Err(error),
            Err(rollback) => Err(error.context(format!(
                "session selection rollback also failed: {rollback:#}"
            ))),
        };
    }
    Ok(())
}

pub(super) enum SelectionUpdate {
    Model {
        model: String,
        expected_provider: Option<String>,
    },
    ReasoningEnabled(bool),
    ReasoningEffort(Option<String>),
}

impl super::AppServerHandle {
    /// Once admitted, persistence and runtime publication have one owner even
    /// when the request disconnects or its calling future is cancelled.
    pub(super) async fn update_selection(&self, update: SelectionUpdate) -> Result<()> {
        let server = self.clone();
        tokio::spawn(async move {
            let _transition = server.settings_transition.lock().await;
            match update {
                SelectionUpdate::Model {
                    model,
                    expected_provider,
                } => {
                    server
                        .runtime
                        .set_model_name_with_commit(
                            model,
                            expected_provider.as_deref(),
                            |config, model, reasoning| {
                                server.persist_selection(config, model, reasoning)
                            },
                        )
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
        if super::session_model::persist_if_scoped(self, &config, &model, &reasoning).await? {
            return Ok(());
        }
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
        let store = store.clone();
        tokio::spawn(async move {
            // The owner retains the global path lock through storage commit and rollback.
            let _guard = guard;
            #[cfg(test)]
            {
                let session = store.session_dir().join(FILE);
                tokio::task::spawn_blocking(move || tests::pause_persistence(&session)).await?;
            }
            persist_pair(&store, last, selection).await
        })
        .await
        .context("selection persistence task failed")?
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
    let selected = if is_new {
        let source = last_path(config, path)?;
        tokio::task::spawn_blocking(move || read_selection(&source, &binding))
            .await
            .context("selection restore worker failed")??
    } else {
        let selected: Option<Selection> = read_session(store, FILE).await?;
        match selected {
            Some(selection) => {
                ensure!(
                    selection.version == 1,
                    "unsupported selection version {}",
                    selection.version
                );
                ensure!(
                    !selection.model.model.trim().is_empty(),
                    "saved model is empty"
                );
                (selection.binding == binding).then_some(selection)
            }
            None => None,
        }
    };
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
                .write_metadata(FILE, serde_json::to_value(&selection)?)
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
