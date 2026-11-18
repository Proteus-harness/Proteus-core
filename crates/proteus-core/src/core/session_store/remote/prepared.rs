//! Startup uses disposable display pages provisionally until strict continuation.
use std::path::{Path, PathBuf};

use anyhow::Result;
use proteus_contracts::app_protocol::AppTranscriptMessage;

use super::{
    RemoteSessionStore, StorageService, catalog,
    snapshot::{RemoteJournalSnapshot, RemoteValidatedRead},
    view::{RemoteCacheSource, SessionReadModel, SessionReadView},
};

pub(crate) struct PreparedSession {
    store: RemoteSessionStore,
    history: PreparedHistory,
    source: Option<RemoteCacheSource>,
    stale: bool,
}

enum PreparedHistory {
    Validated(RemoteValidatedRead),
    Cached(SessionReadView),
}

impl PreparedSession {
    pub(crate) fn from_dir(service: StorageService, session_dir: PathBuf) -> Result<Self> {
        let store = RemoteSessionStore::open(service, session_dir)?;
        if let Some((source, view, fresh)) = store.saved_read_view() {
            return Ok(Self {
                store,
                history: PreparedHistory::Cached(view),
                source: Some(source),
                stale: !fresh,
            });
        }
        let read = RemoteValidatedRead::open_for_display(store.clone())?;
        let source = Some(RemoteCacheSource::from_cursor(&read.cursor()));
        Ok(Self {
            store,
            history: PreparedHistory::Validated(read),
            source,
            stale: false,
        })
    }

    #[cfg(test)]
    pub(crate) fn session_dir(&self) -> &Path {
        self.store.session_dir()
    }

    pub(crate) fn workspace_path(&self) -> Result<PathBuf> {
        self.store.workspace_path()
    }
    pub(crate) fn message_count(&self) -> usize {
        match &self.history {
            PreparedHistory::Validated(read) => read.history().len(),
            PreparedHistory::Cached(view) => view.summary.message_count,
        }
    }

    pub(crate) fn into_display_view(
        self,
        render: impl FnOnce(&mut RemoteJournalSnapshot) -> Result<Vec<AppTranscriptMessage>>,
    ) -> Result<(RemoteSessionStore, SessionReadView, bool, bool)> {
        let (view, deferred) = match self.history {
            PreparedHistory::Validated(read) => {
                let model = SessionReadModel::from_validated(&self.store, read, render)?;
                if let Some(source) = &self.source {
                    self.store.save_read_model(source, &model);
                }
                (
                    SessionReadView::from_model(&self.store, model, false),
                    false,
                )
            }
            PreparedHistory::Cached(view) => (view, true),
        };
        Ok((self.store, view, deferred, self.stale))
    }
}

pub(crate) fn prepare_latest_workspace_session(
    service: StorageService,
    config_root: &Path,
    workspace_path: &Path,
) -> Result<Option<PreparedSession>> {
    let mut candidates = catalog::list(service.clone(), config_root, Some(workspace_path), false)?;
    candidates.retain(|entry| entry.parent_session_id.is_none());
    candidates.sort_by(|left, right| {
        right
            .updated_at_ms
            .cmp(&left.updated_at_ms)
            .then_with(|| right.session_dir.cmp(&left.session_dir))
    });
    for entry in candidates {
        let path = entry.session_dir;
        match PreparedSession::from_dir(service.clone(), path.clone()) {
            Ok(prepared) if prepared.message_count() > 0 || prepared.stale => {
                return Ok(Some(prepared));
            }
            Ok(_) => {}
            Err(error) => eprintln!(
                "warning: cannot prepare session {}: {error:#}",
                path.display()
            ),
        }
    }
    Ok(None)
}
