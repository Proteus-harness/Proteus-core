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
mod publishing;
pub(super) use publishing::PublishingTranscript;
mod startup_history;
mod text_rows;
pub(crate) use cache::RemoteCacheSource;
pub(crate) use prefix::PrefixIdentities;
use startup_history::StartupHistory;
pub(crate) use text_rows::{CompactTextRow, DisplayRows, transcript_role};

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
                model_context: state.model_context,
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
        self.project_startup_view(read, paged, false, |snapshot| {
            render(snapshot).map(DisplayRows::Full)
        })
    }

    pub(super) fn project_startup_view(
        &self,
        read: RemoteValidatedRead,
        paged: bool,
        initial_open: bool,
        render: impl FnOnce(&mut RemoteJournalSnapshot) -> Result<DisplayRows>,
    ) -> Result<SessionReadView> {
        let source = RemoteCacheSource::from_cursor(&read.cursor());
        let (history, transcript, prefix, summary, revision, context, usage) = {
            let (mut snapshot, state) = read.into_parts();
            let mut history = StartupHistory::prepare(
                state.history,
                initial_open && paged && snapshot.has_compact_text_stage(),
            );
            let transcript = render(&mut snapshot)?;
            let mut prefix = snapshot.scan_display_prefix()?;
            prefix.history = history.fingerprints()?;
            let summary = cache::summary_with(self, history.len(), || history.preview())?;
            let context = state.model_context;
            (
                history,
                transcript,
                prefix,
                summary,
                state.history_revision,
                context,
                state.usage,
            )
            // Preserve snapshot drop/release after Describe and before saving.
        };
        let projection = cache::StartupProjection {
            summary: summary.clone(),
            revision,
            context_summary: history.context_summary(),
            history,
            context,
            prefix: Some(prefix),
        };
        if paged
            && initial_open
            && let Some((runtime, permit, revision_key)) = publishing::admit(&transcript, &source)
        {
            let pending = std::sync::Arc::new(PublishingTranscript::new(
                revision_key,
                transcript,
                projection.context_summary.clone(),
                usage.clone(),
                permit,
            ));
            let page = pending.page(None).expect("tail of validated transcript");
            *self.handles.publishing.lock().unwrap() = Some(pending.clone());
            let store = self.clone();
            runtime.spawn_blocking(move || {
                let saved = projection.save(&store, &source, &pending.usage, &pending.rows);
                // If the cache filled up or the journal advanced, the old cursor
                // still addresses its immutable rows for the life of the session.
                if saved.is_some() {
                    store.handles.publishing.lock().unwrap().take();
                }
                drop(pending);
                crate::release_unused_allocator_pages();
            });
            return Ok(SessionReadView {
                summary,
                revision,
                usage,
                transcript: page.messages,
                cursor: Some(AppTranscriptCursor {
                    revision: page.revision,
                    before: page.start,
                    base: None,
                }),
            });
        }
        let _ = projection.save(self, &source, &usage, &transcript);
        // A concurrent projection may have advanced HEAD. Only this immutable
        // revision can provide a page for the model's summary, revision and usage.
        let page = if paged {
            // Keep the complete rendered transcript until the named page is
            // actually available, even when publishing the cache failed.
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
            None => (transcript.into_full(), None),
        };
        Ok(SessionReadView {
            summary,
            revision,
            usage,
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
        if let Some(pending) = self.current_publishing_transcript() {
            return Some(pending.usage.clone());
        }
        Some(cache::fresh_header(self)?.usage)
    }

    pub(crate) fn fresh_context_summary(&self) -> Option<AppContextHistorySummary> {
        if let Some(pending) = self.current_publishing_transcript() {
            return Some(pending.context_summary.clone());
        }
        Some(cache::fresh_header(self)?.context_summary)
    }

    pub(crate) fn saved_transcript_page(
        &self,
        before: Option<usize>,
        revision: Option<&str>,
    ) -> Option<AppTranscriptPage> {
        if let Some(pending) = self.publishing_transcript(revision) {
            let page = pending.page(before);
            if revision.is_some() || RemoteCacheSource::read(self)?.revision()? == pending.revision
            {
                return page;
            }
        }
        cache::transcript_page(self, before, revision, false)
    }

    pub(crate) fn saved_display_page(&self) -> Option<AppTranscriptPage> {
        cache::transcript_page(self, None, None, true)
            .or_else(|| self.publishing_transcript(None)?.page(None))
    }

    pub(crate) fn saved_transcript(&self, revision: &str) -> Option<Vec<AppTranscriptMessage>> {
        if let Some(pending) = self.publishing_transcript(Some(revision)) {
            return Some(pending.rows.copy_range(0..pending.rows.len()));
        }
        cache::load_named_transcript(self, revision)
    }

    pub(crate) fn saved_tool_state(&self, name: &str) -> Option<AppLatestToolState> {
        cache::tool_state(self, name)
    }
}

fn display_prefix(
    snapshot: &mut RemoteJournalSnapshot,
    history: &[crate::model_standard::CanonicalMessage],
) -> Result<PrefixIdentities> {
    let mut prefix = snapshot.scan_display_prefix()?;
    prefix.history = history
        .iter()
        .map(crate::core::history_message_sha256)
        .collect::<Result<_>>()?;
    Ok(prefix)
}
