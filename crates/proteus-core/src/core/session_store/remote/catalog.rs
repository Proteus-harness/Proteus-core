use super::*;
use proteus_contracts::app_protocol::AppSessionSummary;

pub(crate) fn list(
    service: StorageService,
    config_root: &Path,
    workspace_path: Option<&Path>,
    audit: bool,
) -> Result<Vec<AppSessionSummary>> {
    let Response::Sessions { sessions } = service.execute_blocking(Request::List {
        config_root: config_root.to_owned(),
        workspace_path: workspace_path.map(Path::to_owned),
        audit,
    })?
    else {
        bail!("storage returned an unexpected catalog response")
    };
    sessions
        .into_iter()
        .map(|entry| {
            let descriptor = entry.descriptor;
            Ok(AppSessionSummary {
                session_id: descriptor.session.session_id,
                parent_session_id: descriptor.parent_session_id,
                session_dir: descriptor
                    .local_path
                    .ok_or_else(|| anyhow!("storage catalog has no display locator"))?,
                workspace_path: descriptor.workspace_path,
                message_count: entry.message_count,
                preview: entry.preview,
                updated_at_ms: u64::try_from(descriptor.updated_at_ms).ok(),
                activity: None,
            })
        })
        .collect()
}

/// Provisional display discovery. Canonical validation happens after selection.
pub(crate) fn candidates(
    service: StorageService,
    config_root: &Path,
    workspace_path: Option<&Path>,
) -> Result<Vec<proteus_contracts::contracts::StorageSessionDescriptor>> {
    let Response::Candidates { sessions } = service.execute_blocking(Request::ListCandidates {
        config_root: config_root.to_owned(),
        workspace_path: workspace_path.map(Path::to_owned),
    })?
    else {
        bail!("storage returned an unexpected candidate response")
    };
    Ok(sessions)
}

fn updates() -> &'static tokio::sync::watch::Sender<u64> {
    static UPDATES: std::sync::OnceLock<tokio::sync::watch::Sender<u64>> =
        std::sync::OnceLock::new();
    UPDATES.get_or_init(|| tokio::sync::watch::channel(0).0)
}

pub(crate) fn subscribe_updates() -> tokio::sync::watch::Receiver<u64> {
    updates().subscribe()
}
pub(super) fn notify_update() {
    updates().send_modify(|revision| *revision = revision.wrapping_add(1));
}

impl RemoteSessionStore {
    pub(crate) fn validate_owned_delete_scope(
        &self,
        config_root: &Path,
        workspace: &Path,
    ) -> Result<()> {
        let workspace = std::fs::canonicalize(workspace)?;
        ensure!(
            self.descriptor.workspace_path == workspace,
            "session belongs to another workspace"
        );
        let root = crate::core::canonicalize_session_dir_path(config_root.to_owned())?;
        if self.created_root.as_ref() == Some(&root) {
            return Ok(());
        }
        let expected = list(self.service.clone(), &root, Some(&workspace), false)?;
        ensure!(
            expected
                .iter()
                .any(|entry| entry.session_id == self.session_id()
                    && entry.session_dir == self.session_dir()),
            "session is outside the configured workspace catalog"
        );
        Ok(())
    }

    pub(crate) async fn delete_owned(&self, config_root: &Path, workspace: &Path) -> Result<bool> {
        let store = self.clone();
        let root = config_root.to_owned();
        let workspace = workspace.to_owned();
        tokio::task::spawn_blocking(move || store.validate_owned_delete_scope(&root, &workspace))
            .await??;
        self.delete(true).await?;
        Ok(true)
    }
}

pub(crate) async fn delete(
    service: StorageService,
    config_root: &Path,
    workspace_path: &Path,
    path: PathBuf,
) -> Result<bool> {
    let root = config_root.to_owned();
    let cwd = workspace_path.to_owned();
    let store = tokio::task::spawn_blocking(move || {
        let store = RemoteSessionStore::open(service, path)?;
        store.validate_owned_delete_scope(&root, &cwd)?;
        Ok::<_, anyhow::Error>(store)
    })
    .await?;
    match store {
        Ok(store) => {
            store.delete(false).await?;
            Ok(true)
        }
        Err(error)
            if error.downcast_ref::<proteus_contracts::contracts::SessionStorageFailure>()
                == Some(&proteus_contracts::contracts::SessionStorageFailure::MissingSession) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}
