use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow};
use proteus_contracts::app_protocol::{AppBootstrap, http::SessionOpened};
use serde_json::{Value, json};

use super::{commands::command_response, state::HttpAppState};
use crate::{
    app_server::{AgentAppServer, AppServerHandle, StdioOutput},
    core::{canonicalize_session_dir_path, config_store_root, delete_workspace_session},
};

pub(super) async fn bootstrap(state: &HttpAppState) -> AppBootstrap {
    let session_dir = match &state.launch.initial_session_dir {
        Some(path) if state.server_for_session_dir(path).await.is_some() => Some(path.clone()),
        _ => None,
    };
    AppBootstrap {
        session_dir,
        cwd: state.launch.cwd.clone(),
    }
}

pub(super) async fn execute_resume(
    state: &HttpAppState,
    id: Option<String>,
    session_dir: PathBuf,
) -> StdioOutput {
    command_response(id, resume_session(state, session_dir).await.map(Some))
}

pub(super) async fn execute_new_session(
    state: &HttpAppState,
    id: Option<String>,
    source_session_dir: Option<PathBuf>,
    workspace: Option<PathBuf>,
) -> StdioOutput {
    let result = async {
        let workspace = workspace
            .map(|path| {
                let path = std::fs::canonicalize(&path)
                    .with_context(|| format!("workspace {} is unavailable", path.display()))?;
                anyhow::ensure!(
                    path.is_dir(),
                    "workspace {} is not a directory",
                    path.display()
                );
                Ok(path)
            })
            .transpose()?;
        let (config, cwd, config_path, permission_mode) = match source_session_dir {
            Some(path) => {
                let source = super::sessions::server_for_session(state, path).await?;
                let config = source.config.read().await.clone();
                (
                    config,
                    source.cwd.clone(),
                    source.config_path.clone(),
                    Some(source.permission_mode().await),
                )
            }
            None => (
                current_launch_config(state).await?,
                state.launch.cwd.clone(),
                state.launch.config_path.clone(),
                None,
            ),
        };
        ensure_storage_binding(state, &config)?;
        let cwd = workspace.unwrap_or(cwd);
        let next = AgentAppServer::launch_with_storage(
            config,
            cwd,
            config_path.as_deref(),
            state.storage.clone(),
        )
        .await?;
        // Inherit the source session override without changing the published
        // profile that config saves and unrelated reloads use as their baseline.
        if let Some(mode) = permission_mode {
            next.set_permission_mode(mode).await?;
        }
        next.start_session().await?;
        state.remember_server(next.clone()).await;
        session_opened(state, &next).await
    }
    .await;
    command_response(id, result.map(Some))
}

pub(super) async fn execute_delete_session(
    state: &HttpAppState,
    id: Option<String>,
    session_dir: PathBuf,
) -> StdioOutput {
    // The admitted operation owns settlement/removal. Dropping its HTTP waiter
    // must not leave a closed runtime registered or release the path gate while
    // a blocking filesystem worker is still removing storage.
    let state = state.clone();
    let result = tokio::spawn(async move {
        let session_dir = state
            .resolve_session_dir(canonicalize_session_dir_path(session_dir)?)
            .await?;
        let _lifecycle = state.lock_session_lifecycle(&session_dir).await;
        let config_path = state
            .launch
            .config_path
            .as_deref()
            .ok_or_else(|| anyhow!("HTTP sessions require a config path for storage"))?;
        let live_server = state.server_for_session_dir(&session_dir).await;
        let was_live = live_server.is_some();
        let storage = state.storage_service()?;
        let workspace = match &live_server {
            Some(server) => {
                let store = server
                    .runtime
                    .session_store()
                    .cloned()
                    .ok_or_else(|| anyhow!("session has no storage"))?;
                let root = config_store_root(config_path);
                let cwd = server.cwd.clone();
                tokio::task::spawn_blocking(move || store.validate_owned_delete_scope(&root, &cwd))
                    .await??;
                server.cwd.clone()
            }
            None => {
                let root = config_store_root(config_path);
                let selected = storage.clone();
                let path = session_dir.clone();
                let summaries = tokio::task::spawn_blocking(move || {
                    crate::core::list_session_summaries(selected, &root)
                })
                .await??;
                let Some(summary) = summaries
                    .into_iter()
                    .find(|summary| super::super::paths_equal(&summary.session_dir, &path))
                else {
                    return Ok(Some(json!({"deleted": false})));
                };
                summary.workspace_path
            }
        };
        let deleted = if let Some(server) = live_server {
            // Keep the owner's lease throughout terminal settlement. Removing a
            // cache entry is not ownership release: other handles remain valid
            // references to the generation that owned deletion will retire.
            server.shutdown().await;
            state.remove_session_server(&session_dir).await;
            server.delete_closed_session().await?
        } else {
            delete_workspace_session(
                storage,
                &config_store_root(config_path),
                &workspace,
                session_dir,
            )
            .await?
        };
        Ok(Some(json!({ "deleted": deleted || was_live })))
    })
    .await
    .context("session deletion task failed")
    .and_then(|result| result);
    command_response(id, result)
}

async fn resume_session(state: &HttpAppState, session_dir: PathBuf) -> Result<Value> {
    let session_dir = state
        .resolve_session_dir(canonicalize_session_dir_path(session_dir)?)
        .await?;
    let _lifecycle = state.lock_session_lifecycle(&session_dir).await;
    if let Some(existing) = state.server_for_session_dir(&session_dir).await {
        return session_opened(state, &existing).await;
    }
    let launch = &state.launch;
    let config = current_launch_config(state).await?;
    ensure_storage_binding(state, &config)?;
    let next = AgentAppServer::launch_resumed_with_storage(
        config,
        launch.cwd.clone(),
        launch.config_path.as_deref(),
        session_dir,
        state.storage_service()?,
    )
    .await?;
    state.remember_server(next.clone()).await;
    session_opened(state, &next).await
}

async fn session_opened(state: &HttpAppState, server: &AppServerHandle) -> Result<Value> {
    let session_dir = server
        .session_dir_path()
        .ok_or_else(|| anyhow!("live session has no session directory"))?;
    Ok(serde_json::to_value(SessionOpened {
        session_dir,
        workspace_path: server.cwd_path().to_path_buf(),
        activity: state.activity_for_server(server).await,
    })?)
}

pub(super) async fn config_summary_with_activity(
    state: &HttpAppState,
    server: &AppServerHandle,
    discover: bool,
) -> Result<Value> {
    let mut summary = if discover {
        server.config_summary().await
    } else {
        server.local_config_summary().await
    };
    if let Value::Object(fields) = &mut summary {
        fields.insert(
            "activity".to_owned(),
            serde_json::to_value(state.activity_for_server(server).await)?,
        );
    }
    Ok(summary)
}

fn ensure_storage_binding(state: &HttpAppState, config: &crate::core::AppConfig) -> Result<()> {
    anyhow::ensure!(
        crate::core::session_storage_binding(&state.launch.config)?
            == crate::core::session_storage_binding(config)?,
        "session storage configuration changed; restart the HTTP server before opening another session"
    );
    Ok(())
}

/// New admissions reload the published profile before constructing components;
/// existing sessions retain their immutable execution/configuration snapshot.
async fn current_launch_config(state: &HttpAppState) -> Result<crate::core::AppConfig> {
    match state.launch.config_path.as_deref() {
        Some(path) if state.launch.profile_was_present || path.try_exists()? => {
            crate::core::AppConfig::load(Some(path)).await
        }
        Some(_) => Ok(state.launch.config.clone()),
        None => Ok(state.launch.config.clone()),
    }
}
