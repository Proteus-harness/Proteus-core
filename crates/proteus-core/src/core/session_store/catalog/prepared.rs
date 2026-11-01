//! Startup hands off either a strict projection or a disposable saved read model.
use std::path::{Path, PathBuf};

use anyhow::Result;

#[cfg(unix)]
use super::super::read_cache::SourceFingerprint;
use super::super::read_model::{SessionReadModel, SessionReadView};
use crate::core::{
    SessionStore,
    session_journal::{JournalReadSnapshot, ValidatedJournalRead},
};

use super::{session_updated_at_ms, warn_unusable_session, workspace_session_dirs};

pub(crate) struct PreparedSession {
    store: SessionStore,
    history: PreparedHistory,
    #[cfg(unix)]
    source: Option<SourceFingerprint>,
    stale: bool,
}

enum PreparedHistory {
    Validated(ValidatedJournalRead),
    #[cfg(unix)]
    Cached(SessionReadView),
}

impl PreparedSession {
    pub(crate) fn from_dir(session_dir: PathBuf) -> Result<Self> {
        #[cfg(unix)]
        {
            let store = SessionStore::open(session_dir.clone())?;
            if let Some((source, model, fresh)) = store.saved_read_view() {
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
                    PreparedHistory::Validated(read) => super::session_summary_from_history(
                        prepared.session_dir().to_owned(),
                        prepared.store.session_id(),
                        prepared.store.parent_session_id(),
                        read.history(),
                    )?,
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
        let read = ValidatedJournalRead::open(store.session_dir(), store.session_id())?;
        Ok(Self {
            store,
            history: PreparedHistory::Validated(read),
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
            PreparedHistory::Validated(read) => read.history().len(),
            #[cfg(unix)]
            PreparedHistory::Cached(model) => model.summary.message_count,
        }
    }

    pub(crate) fn into_display_view(
        self,
        render: impl FnOnce(
            &mut JournalReadSnapshot,
        )
            -> Result<Vec<proteus_contracts::app_protocol::AppTranscriptMessage>>,
    ) -> Result<(SessionStore, SessionReadView, bool, bool)> {
        #[cfg(feature = "memory-diagnostics")]
        let memory = crate::memory_diagnostics::Scope::new("prepared.into_display_view");
        let (model, deferred) = match self.history {
            PreparedHistory::Validated(read) => {
                let model = SessionReadModel::from_validated(&self.store, read, render)?;
                #[cfg(unix)]
                if let Some(source) = &self.source {
                    self.store.save_read_model(source, &model);
                }
                (
                    SessionReadView::from_model(&self.store, model, false),
                    false,
                )
            }
            #[cfg(unix)]
            PreparedHistory::Cached(model) => (model, true),
        };
        #[cfg(feature = "memory-diagnostics")]
        memory.sample(
            "prepared.full_model_dropped",
            &[("page_messages", model.transcript.len())],
        );
        #[cfg(unix)]
        let store = match self.source {
            Some(_) => self
                .store
                .with_read_metadata(model.revision, model.usage.clone()),
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
        // Inspect only identity metadata before preparing a history/read model.
        // Subagent activity must not become the default main conversation.
        match super::resolve_session_identity(&session_dir) {
            Ok(identity) if identity.parent_session_id.is_some() => continue,
            Ok(_) => {}
            Err(error) => {
                warn_unusable_session(&session_dir, &error);
                continue;
            }
        }
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
