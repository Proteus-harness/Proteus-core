//! Disposable read models. The canonical journal remains the authority for execution.
use serde::{Deserialize, Serialize};

use crate::{
    contracts::ConversationSnapshot,
    core::{JournalProjection, model_context::ModelContextState, session_journal::UsageProjection},
    domain::SessionUsageSnapshot,
};
use proteus_contracts::app_protocol::{AppSessionSummary, AppTranscriptMessage};

use super::SessionStore;

pub(crate) const VIEW_CACHE: &str = "view";

#[cfg(unix)]
mod sqlite;

#[cfg(unix)]
pub(super) use sqlite::conversation as sqlite_conversation;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionReadModel {
    pub summary: AppSessionSummary,
    pub conversation: ConversationSnapshot,
    pub transcript: Vec<AppTranscriptMessage>,
    pub usage: SessionUsageSnapshot,
}

/// An opened view keeps metadata and one display page, not the conversation.
#[derive(Debug)]
pub(crate) struct SessionReadView {
    pub summary: AppSessionSummary,
    pub revision: u64,
    pub usage: SessionUsageSnapshot,
    pub transcript: Vec<AppTranscriptMessage>,
    pub cursor: Option<proteus_contracts::app_protocol::AppTranscriptCursor>,
}

impl SessionReadView {
    pub(crate) fn from_model(store: &SessionStore, model: SessionReadModel, stale: bool) -> Self {
        #[cfg(unix)]
        let page = if stale {
            store.saved_display_page()
        } else {
            store.saved_transcript_page(None, None)
        };
        #[cfg(not(unix))]
        let page: Option<proteus_contracts::app_protocol::AppTranscriptPage> = None;
        let (transcript, cursor) = match page {
            Some(page) => (
                page.messages,
                Some(proteus_contracts::app_protocol::AppTranscriptCursor {
                    revision: page.revision,
                    before: page.start,
                    base: page.base,
                }),
            ),
            None => (model.transcript, None),
        };
        Self {
            summary: model.summary,
            revision: model.conversation.revision,
            usage: model.usage,
            transcript,
            cursor,
        }
    }
}

impl SessionReadModel {
    pub(crate) fn from_projection(
        store: &SessionStore,
        projection: JournalProjection,
        transcript: Vec<AppTranscriptMessage>,
    ) -> anyhow::Result<Self> {
        let summary = super::catalog::session_summary_from_projection(
            store.session_dir().to_owned(),
            store.session_id(),
            &projection,
        )?;
        let mut usage = UsageProjection::default();
        for record in &projection.records {
            usage.apply(record);
        }
        let model_context = ModelContextState::from_records(&projection.records, None).snapshot();
        Ok(Self {
            summary,
            conversation: ConversationSnapshot {
                revision: projection.history_revision,
                messages: projection.history,
                model_context,
            },
            transcript,
            usage: usage.snapshot(store.session_id()),
        })
    }

    pub(crate) fn matches(&self, store: &SessionStore) -> bool {
        self.summary.session_id == store.session_id()
            && self.usage.session_id == store.session_id()
            && self.summary.session_dir == store.session_dir()
            && store
                .workspace_path()
                .is_ok_and(|path| path == self.summary.workspace_path)
            && self.summary.message_count == self.conversation.messages.len()
    }
}

impl SessionStore {
    /// Current read projection only; stale display snapshots are a separate API.
    pub(crate) fn fresh_read_model(&self) -> Option<SessionReadModel> {
        #[cfg(unix)]
        if let Some((_, model, true)) = sqlite::load(self)
            && model.matches(self)
        {
            return Some(model);
        }
        None
    }

    pub(crate) fn project_read_model(
        &self,
        render: impl FnOnce(&JournalProjection) -> Vec<AppTranscriptMessage>,
    ) -> anyhow::Result<SessionReadModel> {
        #[cfg(unix)]
        let source = super::read_cache::SourceFingerprint::read(self.session_dir(), 8192);
        let projection = self.load_projection()?;
        let transcript = render(&projection);
        let model = SessionReadModel::from_projection(self, projection, transcript)?;
        #[cfg(unix)]
        if let Some(source) = source {
            self.save_read_model(&source, &model);
        }
        Ok(model)
    }

    #[cfg(unix)]
    pub(crate) fn save_read_model(
        &self,
        source: &super::read_cache::SourceFingerprint,
        model: &SessionReadModel,
    ) {
        sqlite::save(self, source, model);
        super::read_cache::save(self.session_dir(), "summary", source, &model.summary);
    }

    #[cfg(unix)]
    pub(crate) fn saved_read_view(
        &self,
    ) -> Option<(super::read_cache::SourceFingerprint, SessionReadView, bool)> {
        sqlite::view(self)
    }

    #[cfg(unix)]
    pub(crate) fn fresh_context_summary(
        &self,
    ) -> Option<proteus_contracts::app_protocol::AppContextHistorySummary> {
        sqlite::context_summary(self)
    }

    #[cfg(unix)]
    pub(crate) fn fresh_read_usage(&self) -> Option<SessionUsageSnapshot> {
        sqlite::usage(self)
    }

    #[cfg(unix)]
    pub(crate) fn saved_transcript_page(
        &self,
        before: Option<usize>,
        revision: Option<&str>,
    ) -> Option<proteus_contracts::app_protocol::AppTranscriptPage> {
        sqlite::page(self, before, revision)
    }
    #[cfg(unix)]
    pub(crate) fn saved_display_page(
        &self,
    ) -> Option<proteus_contracts::app_protocol::AppTranscriptPage> {
        sqlite::display_page(self)
    }

    #[cfg(unix)]
    pub(crate) fn saved_transcript(&self, revision: &str) -> Option<Vec<AppTranscriptMessage>> {
        sqlite::transcript(self, revision)
    }
}

#[cfg(test)]
mod tests;

#[cfg(unix)]
#[derive(Debug, Clone)]
pub(super) struct CachedReadMetadata {
    pub revision: u64,
    pub usage: SessionUsageSnapshot,
}

#[cfg(unix)]
impl SessionStore {
    pub(crate) fn with_read_metadata(self, revision: u64, usage: SessionUsageSnapshot) -> Self {
        self.set_read_metadata(revision, usage);
        self
    }

    pub(super) fn cached_read_metadata(&self) -> Option<CachedReadMetadata> {
        self.read_metadata
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn set_read_metadata(&self, revision: u64, usage: SessionUsageSnapshot) {
        *self
            .read_metadata
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(CachedReadMetadata { revision, usage });
    }
}
