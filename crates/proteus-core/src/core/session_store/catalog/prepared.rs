//! Startup hands off either a strict projection or a disposable saved read model.
use std::path::{Path, PathBuf};

use anyhow::Result;

#[cfg(unix)]
use super::super::read_cache::SourceFingerprint;
use super::super::read_model::SessionReadModel;
use crate::core::{SessionStore, session_journal::JournalProjection};

use super::{session_updated_at_ms, warn_unusable_session, workspace_session_dirs};

pub(crate) struct PreparedSession {
    store: SessionStore,
    history: PreparedHistory,
    #[cfg(unix)]
    source: Option<SourceFingerprint>,
    stale: bool,
}

enum PreparedHistory {
    Validated(JournalProjection),
    #[cfg(unix)]
    Cached(SessionReadModel),
}

impl PreparedSession {
    pub(crate) fn from_dir(session_dir: PathBuf) -> Result<Self> {
        #[cfg(unix)]
        {
            let store = SessionStore::open(session_dir.clone())?;
            if let Some((source, model, fresh)) = store.saved_read_model()
                && model.matches(&store)
            {
                return Ok(Self {
                    store,
                    history: PreparedHistory::Cached(model),
                    source: Some(source),
                    stale: !fresh,
                });
            }
        }
        #[cfg(unix)]
        {
            super::cache::prime_from_read(&session_dir, || {
                let prepared = Self::load(&session_dir)?;
                let summary = match &prepared.history {
                    PreparedHistory::Validated(projection) => {
                        super::session_summary_from_projection(
                            prepared.session_dir().to_owned(),
                            prepared.store.session_id(),
                            projection,
                        )?
                    }
                    PreparedHistory::Cached(model) => model.summary.clone(),
                };
                Ok((prepared, summary))
            })
        }
        #[cfg(not(unix))]
        Self::load(&session_dir)
    }

    fn load(session_dir: &Path) -> Result<Self> {
        let store = SessionStore::open(session_dir.to_owned())?;
        #[cfg(unix)]
        let source = SourceFingerprint::read(session_dir, 8192);
        let projection = store.load_projection()?;
        Ok(Self {
            store,
            history: PreparedHistory::Validated(projection),
            stale: false,
            #[cfg(unix)]
            source: source.filter(|before| {
                SourceFingerprint::read(session_dir, 8192).as_ref() == Some(before)
            }),
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
            PreparedHistory::Validated(projection) => projection.history.len(),
            #[cfg(unix)]
            PreparedHistory::Cached(model) => model.conversation.messages.len(),
        }
    }

    pub(crate) fn into_read_model(
        self,
        render: impl FnOnce(
            &JournalProjection,
        ) -> Vec<proteus_contracts::app_protocol::AppTranscriptMessage>,
    ) -> Result<(SessionStore, SessionReadModel, bool, bool)> {
        let (model, deferred) = match self.history {
            PreparedHistory::Validated(projection) => {
                let transcript = render(&projection);
                (
                    SessionReadModel::from_projection(&self.store, projection, transcript)?,
                    false,
                )
            }
            #[cfg(unix)]
            PreparedHistory::Cached(model) => (model, true),
        };
        #[cfg(unix)]
        let store = match self.source {
            Some(source) => {
                if !deferred {
                    self.store.save_read_model(&source, &model);
                }
                self.store.with_read_metadata(&model)
            }
            None => self.store,
        };
        #[cfg(not(unix))]
        let store = self.store;
        Ok((store, model, deferred, self.stale))
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
            // A previously empty view may have been populated by another client.
            // Display it provisionally and let the background refresh reconcile it.
            Ok(prepared) if prepared.message_count() > 0 || prepared.stale => {
                return Ok(Some(prepared));
            }
            Ok(_) => {}
            Err(error) => warn_unusable_session(&session_dir, &error),
        }
    }
    Ok(None)
}
