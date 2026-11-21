//! Session persistence is provided by the selected external storage service.
use crate::{
    contracts::SessionStorage,
    core::{AppConfig, ModuleCatalog},
};
use anyhow::{Result, anyhow};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
mod context_summary;
mod json_size;
mod path_identity;
pub(crate) mod remote;
mod workspace_dir;
pub(crate) use context_summary::summarize_context_history;
pub use remote::RemoteSessionStore as SessionStore;
pub use remote::StorageService;
pub(crate) use remote::{
    prepared::{PreparedSession, prepare_latest_workspace_session},
    snapshot::DisplayRecords,
    view::{CommittedDisplay, PrefixIdentities, SessionReadModel, SessionTranscriptAppend},
};
pub use workspace_dir::{decode_workspace_path, encode_workspace_path};
pub(crate) mod read_cache {
    pub(crate) use super::remote::view::RemoteCacheSource as SourceFingerprint;
}
pub fn configured_session_storage(
    config: &AppConfig,
    cwd: &Path,
) -> Result<Arc<dyn SessionStorage>> {
    ModuleCatalog::selected_session_storage(config, cwd)?
        .ok_or_else(|| anyhow!("no session storage module is selected"))
}
pub(crate) fn session_storage_binding(config: &AppConfig) -> Result<Option<serde_json::Value>> {
    let Some(id) = config.modules.session_storage.as_deref() else {
        return Ok(None);
    };
    let slot = "session_storage";
    let components: Vec<_> = config
        .components
        .iter()
        .filter(|(_, component)| {
            component
                .exports()
                .any(|(kind, export, _)| kind == slot && export == id)
        })
        .map(|(name, component)| Ok((name, serde_json::to_value(component.execution_identity())?)))
        .collect::<Result<_>>()?;
    Ok(Some(serde_json::json!({
        "id": id,
        "components": components,
        "config": config.process_export_config(slot, id)?,
    })))
}
pub fn list_session_summaries(
    service: StorageService,
    root: &Path,
) -> Result<Vec<proteus_contracts::app_protocol::AppSessionSummary>> {
    remote::catalog::list(service, root, None, false)
}
pub fn list_workspace_session_summaries(
    service: StorageService,
    root: &Path,
    cwd: &Path,
) -> Result<Vec<proteus_contracts::app_protocol::AppSessionSummary>> {
    remote::catalog::list(service, root, Some(cwd), false)
}
pub fn list_session_summaries_for_audit(
    service: StorageService,
    root: &Path,
    cwd: Option<&Path>,
) -> Result<Vec<proteus_contracts::app_protocol::AppSessionSummary>> {
    remote::catalog::list(service, root, cwd, true)
}
pub async fn delete_workspace_session(
    service: StorageService,
    root: &Path,
    cwd: &Path,
    path: PathBuf,
) -> Result<bool> {
    remote::catalog::delete(service, root, cwd, path).await
}
pub fn canonicalize_session_dir_path(path: PathBuf) -> Result<PathBuf> {
    path_identity::canonicalize_unmaterialized_path(&path)
}
pub(crate) fn subscribe_catalog_updates() -> tokio::sync::watch::Receiver<u64> {
    remote::catalog::subscribe_updates()
}

#[cfg(test)]
mod checkpoint_tests;
