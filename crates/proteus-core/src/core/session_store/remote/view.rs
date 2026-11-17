//! Disposable display projections over immutable backend snapshots.
use anyhow::Result;
use proteus_contracts::{
    app_protocol::{
        AppContextHistorySummary, AppLatestToolState, AppSessionSummary, AppTranscriptCursor,
        AppTranscriptMessage, AppTranscriptPage,
    },
    contracts::ConversationSnapshot,
    domain::SessionUsageSnapshot,
};
use serde::{Deserialize, Serialize};

use super::{
    RemoteSessionStore,
    snapshot::{RemoteJournalSnapshot, RemoteValidatedRead},
};

mod cache;
mod incremental;
pub(crate) use cache::RemoteCacheSource;

#[cfg(test)]
mod tests;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionReadModel {
    pub summary: AppSessionSummary,
    pub conversation: ConversationSnapshot,
    pub transcript: Vec<AppTranscriptMessage>,
    pub usage: SessionUsageSnapshot,
}

#[derive(Debug)]
pub(crate) struct SessionReadView {
    pub summary: AppSessionSummary,
    pub revision: u64,
    pub usage: SessionUsageSnapshot,
    pub transcript: Vec<AppTranscriptMessage>,
    pub cursor: Option<AppTranscriptCursor>,
}

#[derive(Debug)]
pub(crate) struct SessionTranscriptAppend {
    pub transcript: Vec<AppTranscriptMessage>,
    pub conversation_appended: usize,
}

impl SessionReadView {
    pub(crate) fn from_model(
        store: &RemoteSessionStore,
        model: SessionReadModel,
        stale: bool,
    ) -> Self {
        let page = if stale {
            store.saved_display_page()
        } else {
            store.saved_transcript_page(None, None)
        };
        let (transcript, cursor) = match page {
            Some(page) => (
                page.messages,
                Some(AppTranscriptCursor {
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
    pub(crate) fn from_validated(
        store: &RemoteSessionStore,
        read: RemoteValidatedRead,
        render: impl FnOnce(&mut RemoteJournalSnapshot) -> Result<Vec<AppTranscriptMessage>>,
    ) -> Result<Self> {
        let (mut snapshot, state) = read.into_parts();
        let transcript = render(&mut snapshot)?;
        Ok(Self {
            summary: cache::summary(store, &state.history)?,
            conversation: ConversationSnapshot {
                revision: state.history_revision,
                messages: state.history,
                model_context: state.model_context.snapshot(),
            },
            transcript,
            usage: state.usage,
        })
    }

    pub(crate) fn matches(&self, store: &RemoteSessionStore) -> bool {
        self.summary.session_id == store.session_id()
            && self.summary.parent_session_id == store.parent_session_id()
            && self.summary.session_dir == store.session_dir()
            && self.usage.session_id == store.session_id()
            && self.summary.message_count == self.conversation.messages.len()
            && store
                .workspace_path()
                .is_ok_and(|path| path == self.summary.workspace_path)
    }
}

impl RemoteSessionStore {
    pub(crate) fn fresh_read_model(&self) -> Option<SessionReadModel> {
        cache::load_model(self)
    }

    pub(crate) fn project_read_model(
        &self,
        render: impl FnOnce(&mut RemoteJournalSnapshot) -> Result<Vec<AppTranscriptMessage>>,
    ) -> Result<SessionReadModel> {
        let read = RemoteValidatedRead::open(self.clone())?;
        let source = RemoteCacheSource::from_cursor(&read.cursor());
        let model = SessionReadModel::from_validated(self, read, render)?;
        self.save_read_model(&source, &model);
        Ok(model)
    }

    pub(crate) fn save_read_model(&self, source: &RemoteCacheSource, model: &SessionReadModel) {
        let _ = cache::save_model(self, source, model, None);
    }

    pub(crate) fn saved_read_view(&self) -> Option<(RemoteCacheSource, SessionReadView, bool)> {
        cache::load_view(self)
    }

    pub(crate) fn fresh_read_usage(&self) -> Option<SessionUsageSnapshot> {
        Some(cache::fresh_header(self)?.usage)
    }

    pub(crate) fn fresh_context_summary(&self) -> Option<AppContextHistorySummary> {
        let header = cache::fresh_header(self)?;
        Some(header.context_summary)
    }

    pub(crate) fn saved_transcript_page(
        &self,
        before: Option<usize>,
        revision: Option<&str>,
    ) -> Option<AppTranscriptPage> {
        cache::transcript_page(self, before, revision, false)
    }

    pub(crate) fn saved_display_page(&self) -> Option<AppTranscriptPage> {
        cache::transcript_page(self, None, None, true)
    }

    pub(crate) fn saved_transcript(&self, revision: &str) -> Option<Vec<AppTranscriptMessage>> {
        let header = cache::header_at_revision(self, revision)?;
        cache::read_section(self, &header.transcript, 0, header.transcript.rows)
    }

    pub(crate) fn saved_tool_state(&self, name: &str) -> Option<AppLatestToolState> {
        cache::tool_state(self, name)
    }
}
