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
mod committed;
mod incremental;
pub(crate) use committed::CommittedDisplay;
mod prefix;
mod text_rows;
pub(crate) use cache::RemoteCacheSource;
pub(crate) use prefix::PrefixIdentities;
pub(crate) use text_rows::{CompactTextRow, DisplayRows};

#[cfg(test)]
mod tests;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionReadModel<T = Vec<AppTranscriptMessage>> {
    pub summary: AppSessionSummary,
    pub conversation: ConversationSnapshot,
    pub transcript: T,
    pub usage: SessionUsageSnapshot,
    #[serde(skip)]
    pub(super) prefix: Option<PrefixIdentities>,
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

impl<T> SessionReadModel<T> {
    pub(crate) fn from_validated(
        store: &RemoteSessionStore,
        read: RemoteValidatedRead,
        render: impl FnOnce(&mut RemoteJournalSnapshot) -> Result<T>,
    ) -> Result<Self> {
        let (mut snapshot, state) = read.into_parts();
        let transcript = render(&mut snapshot)?;
        let prefix = display_prefix(&mut snapshot, &state.history)?;
        Ok(Self {
            summary: cache::summary(store, &state.history)?,
            conversation: ConversationSnapshot {
                revision: state.history_revision,
                messages: state.history,
                model_context: state.model_context.snapshot(),
            },
            transcript,
            usage: state.usage,
            prefix: Some(prefix),
        })
    }
}

impl SessionReadModel {
    fn matches_parts(
        store: &RemoteSessionStore,
        summary: &AppSessionSummary,
        usage: &SessionUsageSnapshot,
        message_count: usize,
    ) -> bool {
        summary.session_id == store.session_id()
            && summary.parent_session_id == store.parent_session_id()
            && summary.session_dir == store.session_dir()
            && usage.session_id == store.session_id()
            && summary.message_count == message_count
            && store
                .workspace_path()
                .is_ok_and(|path| path == summary.workspace_path)
    }
}

impl RemoteSessionStore {
    pub(crate) fn project_validated_view(
        &self,
        read: RemoteValidatedRead,
        paged: bool,
        render: impl FnOnce(&mut RemoteJournalSnapshot) -> Result<Vec<AppTranscriptMessage>>,
    ) -> Result<SessionReadView> {
        self.project_startup_view(read, paged, |snapshot| {
            render(snapshot).map(DisplayRows::Full)
        })
    }

    pub(super) fn project_startup_view(
        &self,
        read: RemoteValidatedRead,
        paged: bool,
        render: impl FnOnce(&mut RemoteJournalSnapshot) -> Result<DisplayRows>,
    ) -> Result<SessionReadView> {
        let source = RemoteCacheSource::from_cursor(&read.cursor());
        let mut model = SessionReadModel::from_validated(self, read, render)?;
        let _ = cache::save_startup_model(self, &source, &mut model);
        let revision = model.conversation.revision;
        // A concurrent projection may have advanced HEAD. Only this immutable
        // revision can provide a page for the model's summary, revision and usage.
        let page = if paged {
            // Keep the complete rendered transcript until the named page is
            // actually available, even when publishing the cache failed.
            drop(model.prefix);
            source
                .revision()
                .and_then(|revision| self.saved_transcript_page(None, Some(&revision)))
        } else {
            None
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
            None => (model.transcript.into_full(), None),
        };
        Ok(SessionReadView {
            summary: model.summary,
            revision,
            usage: model.usage,
            transcript,
            cursor,
        })
    }

    pub(crate) fn fresh_read_model(&self) -> Option<SessionReadModel> {
        cache::load_model(self)
    }

    pub(crate) fn fresh_read_transcript(&self) -> Option<Vec<AppTranscriptMessage>> {
        cache::load_transcript(self)
    }

    pub(crate) fn project_read_model(
        &self,
        render: impl FnOnce(&mut RemoteJournalSnapshot) -> Result<Vec<AppTranscriptMessage>>,
    ) -> Result<SessionReadModel> {
        self.project_sourced_read_model(None, render)
            .map(|(_, model)| model)
    }

    /// Report the source actually validated and rendered. If the caller is
    /// repairing a named revision, do not publish a different projection.
    pub(crate) fn project_sourced_read_model(
        &self,
        expected: Option<&RemoteCacheSource>,
        render: impl FnOnce(&mut RemoteJournalSnapshot) -> Result<Vec<AppTranscriptMessage>>,
    ) -> Result<(RemoteCacheSource, SessionReadModel)> {
        let read = RemoteValidatedRead::open_for_display(self.clone())?;
        let source = RemoteCacheSource::from_cursor(&read.cursor());
        anyhow::ensure!(
            expected.is_none_or(|expected| expected == &source),
            "history source changed during recovery; refresh the session snapshot"
        );
        let model = SessionReadModel::from_validated(self, read, render)?;
        self.save_read_model(&source, &model);
        Ok((source, model))
    }

    pub(crate) fn save_read_model(&self, source: &RemoteCacheSource, model: &SessionReadModel) {
        let _ = cache::save_model(self, source, model, None);
    }

    pub(crate) fn saved_read_view(&self) -> Option<(RemoteCacheSource, SessionReadView)> {
        cache::load_saved_view(self)
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

    #[cfg(test)]
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

fn display_prefix(
    snapshot: &mut RemoteJournalSnapshot,
    history: &[crate::model_standard::CanonicalMessage],
) -> Result<PrefixIdentities> {
    let mut prefix = PrefixIdentities::default();
    snapshot.scan_display(|record| {
        prefix.visit_owned_prefix(record);
        Ok(())
    })?;
    prefix.history = history
        .iter()
        .map(crate::core::history_message_sha256)
        .collect::<Result<_>>()?;
    Ok(prefix)
}
