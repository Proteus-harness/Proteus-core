use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use super::{AgentAppServer, AppServerHandle};
use crate::core::AppConfig;
use anyhow::Result;

pub(super) async fn launch_selected(
    config: Arc<AppConfig>,
    cwd: PathBuf,
    config_path: Option<&Path>,
    resume: Option<PathBuf>,
    fresh: bool,
) -> Result<AppServerHandle> {
    anyhow::ensure!(
        !(fresh && resume.is_some()),
        "fresh session conflicts with explicit resume"
    );
    if let Some(session_dir) = resume {
        AgentAppServer::launch_resumed_shared(config, cwd, config_path, session_dir).await
    } else if fresh {
        AgentAppServer::launch_shared(config, cwd, config_path).await
    } else {
        AgentAppServer::launch_or_resume_latest_shared(config, cwd, config_path).await
    }
}

/// Select an initial HTTP handle without coupling the supervisor's default
/// workspace to a best-effort saved chat. Explicit CLI choices stay strict.
pub(super) async fn launch_http_selected(
    config: AppConfig,
    cwd: PathBuf,
    config_path: Option<&Path>,
    resume: Option<PathBuf>,
    fresh: bool,
    preferred: Option<PathBuf>,
) -> Result<(AppServerHandle, PathBuf)> {
    let Some(path) = preferred.filter(|p| p.is_absolute() && resume.is_none() && !fresh) else {
        let server = launch_selected(config.into(), cwd, config_path, resume, fresh).await?;
        let workspace = server.cwd.clone();
        return Ok((server, workspace));
    };

    let config = Arc::new(config);
    let storage = super::selected_storage(config.clone(), &cwd)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no session storage module is selected"))?;
    let attempt = async {
        let selected = storage.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            let prepared = crate::core::PreparedSession::from_dir(selected, path.clone())?;
            // Desktop accepts exactly the saved directory, not a journal alias.
            anyhow::ensure!(
                prepared.session_dir() == path,
                "not the saved session directory"
            );
            Ok::<_, anyhow::Error>(prepared)
        })
        .await??;
        AgentAppServer::launch_prepared_with_storage(
            config.clone(),
            cwd.clone(),
            config_path,
            prepared,
            storage.clone(),
        )
        .await
    }
    .await;
    let server = match attempt {
        Ok(server) => server,
        Err(error) => {
            eprintln!("warning: could not restore preferred HTTP session: {error:#}");
            AgentAppServer::launch_or_resume_latest_with_storage(
                config,
                cwd.clone(),
                config_path,
                Some(storage),
            )
            .await?
        }
    };
    Ok((server, cwd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fresh_start_ignores_previous_history_while_default_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let config = crate::test_model::config();
        let original = launch_selected(
            config.clone().into(),
            dir.path().to_owned(),
            Some(&config_path),
            None,
            true,
        )
        .await
        .unwrap();
        original
            .send_user_message("previous task".into())
            .await
            .unwrap();
        let old_id = original.session_id();
        let resumed = launch_selected(
            config.clone().into(),
            dir.path().to_owned(),
            Some(&config_path),
            None,
            false,
        )
        .await
        .unwrap();
        assert_eq!(resumed.session_id(), old_id);
        assert!(!resumed.runtime.history().await.unwrap().is_empty());
        let fresh = launch_selected(
            config.clone().into(),
            dir.path().to_owned(),
            Some(&config_path),
            None,
            true,
        )
        .await
        .unwrap();
        assert_ne!(fresh.session_id(), old_id);
        assert!(fresh.runtime.history().await.unwrap().is_empty());
        assert!(
            launch_selected(
                config.into(),
                dir.path().to_owned(),
                Some(&config_path),
                original.session_dir_path(),
                true
            )
            .await
            .is_err()
        );
    }
}
