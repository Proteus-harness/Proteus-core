//! Startup uses disposable display pages provisionally until strict continuation.
use std::path::{Path, PathBuf};

use anyhow::Result;
#[cfg(test)]
use proteus_contracts::app_protocol::AppTranscriptMessage;

use super::{
    RemoteSessionStore, StorageService, catalog,
    snapshot::{RemoteJournalSnapshot, RemoteValidatedRead},
    view::{DisplayRows, RemoteCacheSource, SessionReadView},
};

pub(crate) struct PreparedSession {
    store: RemoteSessionStore,
    history: PreparedHistory,
}

enum PreparedHistory {
    Validated(RemoteValidatedRead),
    Cached(RemoteCacheSource, SessionReadView),
}

impl PreparedSession {
    pub(crate) fn from_dir(service: StorageService, session_dir: PathBuf) -> Result<Self> {
        let store = RemoteSessionStore::open(service, session_dir)?;
        if let Some((source, view)) = store.saved_read_view() {
            return Ok(Self {
                store,
                history: PreparedHistory::Cached(source, view),
            });
        }
        Self::validate_display(store)
    }

    fn validate_display(store: RemoteSessionStore) -> Result<Self> {
        let read = RemoteValidatedRead::open_for_display(store.clone())?;
        Ok(Self {
            store,
            history: PreparedHistory::Validated(read),
        })
    }

    pub(crate) fn session_dir(&self) -> &Path {
        self.store.session_dir()
    }

    pub(crate) fn workspace_path(&self) -> Result<PathBuf> {
        self.store.workspace_path()
    }
    pub(crate) fn message_count(&self) -> usize {
        match &self.history {
            PreparedHistory::Validated(read) => read.history().len(),
            PreparedHistory::Cached(_, view) => view.summary.message_count,
        }
    }

    #[cfg(test)]
    pub(crate) fn into_display_view(
        self,
        render: impl FnOnce(&mut RemoteJournalSnapshot) -> Result<Vec<AppTranscriptMessage>>,
    ) -> Result<(
        RemoteSessionStore,
        SessionReadView,
        Option<RemoteCacheSource>,
    )> {
        self.into_startup_view(|snapshot| render(snapshot).map(DisplayRows::Full))
    }

    pub(crate) fn into_startup_view(
        self,
        render: impl FnOnce(&mut RemoteJournalSnapshot) -> Result<DisplayRows>,
    ) -> Result<(
        RemoteSessionStore,
        SessionReadView,
        Option<RemoteCacheSource>,
    )> {
        let (view, cached_source) = match self.history {
            PreparedHistory::Validated(read) => {
                (self.store.project_startup_view(read, true, render)?, None)
            }
            PreparedHistory::Cached(source, view) => (view, Some(source)),
        };
        Ok((self.store, view, cached_source))
    }
}

struct StartupCandidate {
    updated_at_ms: i64,
    path: PathBuf,
}

fn order_startup_candidates(candidates: &mut [StartupCandidate]) {
    use std::cmp::Ordering;

    let order = |left: &StartupCandidate, right: &StartupCandidate| {
        right
            .updated_at_ms
            .cmp(&left.updated_at_ms)
            .then_with(|| right.path.cmp(&left.path))
    };
    // A provider can already emit the required order. Verify that with the
    // Core comparator (including path ties), never trust provider ordering.
    // For small catalogs, keep the original sort without another pass.
    if candidates.len() >= 32
        && candidates.is_sorted_by(|left, right| order(left, right) != Ordering::Greater)
    {
        return;
    }
    candidates.sort_by(order);
}

pub(crate) fn prepare_latest_workspace_session(
    service: StorageService,
    config_root: &Path,
    workspace_path: &Path,
) -> Result<Option<PreparedSession>> {
    let mut candidates: Vec<_> =
        catalog::candidates(service.clone(), config_root, Some(workspace_path))?
            .into_iter()
            .filter(|entry| entry.parent_session_id.is_none())
            .filter_map(|entry| {
                Some(StartupCandidate {
                    updated_at_ms: entry.updated_at_ms,
                    path: entry.local_path?,
                })
            })
            .collect();
    // In-place collection can keep the full descriptor buffer's capacity.
    candidates.shrink_to_fit();
    order_startup_candidates(&mut candidates);
    for StartupCandidate { path, .. } in candidates {
        let prepared = PreparedSession::from_dir(service.clone(), path.clone()).and_then(|prepared| {
            // An empty saved page can predate an external append. Check the
            // current journal before deciding to skip this candidate.
            if matches!(&prepared.history, PreparedHistory::Cached(_, view) if view.summary.message_count == 0) {
                PreparedSession::validate_display(prepared.store)
            } else {
                Ok(prepared)
            }
        });
        match prepared {
            Ok(prepared) if prepared.message_count() > 0 => return Ok(Some(prepared)),
            Ok(_) => {}
            Err(error) => eprintln!(
                "warning: cannot prepare session {}: {error:#}",
                path.display()
            ),
        }
    }
    Ok(None)
}
