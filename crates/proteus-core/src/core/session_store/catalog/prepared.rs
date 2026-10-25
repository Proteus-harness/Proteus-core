//! A validated startup handoff: the selected journal is read exactly once.
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::core::{SessionStore, session_journal::JournalProjection};

use super::{session_updated_at_ms, warn_unusable_session, workspace_session_dirs};

pub(crate) struct PreparedSession {
    store: SessionStore,
    projection: JournalProjection,
}

impl PreparedSession {
    pub(crate) fn from_dir(session_dir: PathBuf) -> Result<Self> {
        #[cfg(unix)]
        {
            super::cache::prime_from_read(&session_dir, || {
                let prepared = Self::load(&session_dir)?;
                let summary = super::session_summary_from_projection(
                    prepared.session_dir().to_owned(),
                    prepared.store.session_id(),
                    &prepared.projection,
                )?;
                Ok((prepared, summary))
            })
        }
        #[cfg(not(unix))]
        Self::load(&session_dir)
    }

    fn load(session_dir: &Path) -> Result<Self> {
        let store = SessionStore::open(session_dir.to_owned())?;
        let projection = store.load_projection()?;
        Ok(Self { store, projection })
    }

    pub(crate) fn session_dir(&self) -> &Path {
        self.store.session_dir()
    }

    pub(crate) fn workspace_path(&self) -> Result<PathBuf> {
        self.store.workspace_path()
    }

    pub(crate) fn into_parts(self) -> (SessionStore, JournalProjection) {
        (self.store, self.projection)
    }
}

pub(crate) fn prepare_latest_workspace_session(
    config_root: &Path,
    workspace_path: &Path,
) -> Result<Option<PreparedSession>> {
    // Match the catalog's Option<u64> millisecond ordering, including ties.
    // Only the selected usable entry needs journal/blob decoding and validation.
    let mut candidates = workspace_session_dirs(config_root, workspace_path)?
        .into_iter()
        .map(|dir| (session_updated_at_ms(&super::journal_path(&dir)), dir))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.cmp(left));
    for (_, session_dir) in candidates {
        match PreparedSession::from_dir(session_dir.clone()) {
            Ok(prepared) if !prepared.projection.history.is_empty() => return Ok(Some(prepared)),
            Ok(_) => {}
            Err(error) => warn_unusable_session(&session_dir, &error),
        }
    }
    Ok(None)
}
