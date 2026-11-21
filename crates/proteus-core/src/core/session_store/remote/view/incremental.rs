//! Canonical validation remains complete; only certified display suffixes append.
use super::*;
use crate::core::session_store::remote::snapshot::RemoteJournalCursor;
use proteus_contracts::app_protocol::AppTranscriptBase;

impl RemoteSessionStore {
    pub(crate) fn project_read_view(
        &self,
        mut full_renderer: impl FnMut(&mut RemoteJournalSnapshot) -> Result<Vec<AppTranscriptMessage>>,
        mut append_renderer: impl FnMut(
            &mut RemoteJournalSnapshot,
            &RemoteJournalCursor,
            &[[u8; 32]],
            usize,
        ) -> Result<Option<SessionTranscriptAppend>>,
    ) -> Result<SessionReadView> {
        let previous = cache::header(self).filter(|header| header.cursor.is_some());
        let (mut read, prefix_matches) = RemoteValidatedRead::open_with_cursor(
            self.clone(),
            previous.as_ref().and_then(|header| header.cursor.as_ref()),
        )?;
        let cursor = read.cursor();
        let source = RemoteCacheSource::from_cursor(&cursor);
        let baseline = read.take_prefix_history_fingerprints();
        let (mut snapshot, state) = read.into_parts();
        if let (true, Some(previous), Some(baseline)) =
            (prefix_matches, previous.as_ref(), baseline.as_deref())
            && baseline.len() == previous.conversation.rows
            && let Some(suffix) = append_renderer(
                &mut snapshot,
                previous.cursor.as_ref().expect("filtered cursor"),
                baseline,
                state.history.len(),
            )?
            && state.history.len().checked_sub(previous.conversation.rows)
                == Some(suffix.conversation_appended)
        {
            let mut publish = || {
                let mut next = previous.clone();
                next.source = source.clone();
                next.summary = cache::summary(self, &state.history).ok()?;
                next.prefix = display_prefix(&mut snapshot, &state.history)
                    .ok()
                    .and_then(|index| cache::write_prefix(self, &source, &index));
                next.revision = state.history_revision;
                next.usage = state.usage.clone();
                next.context_summary = crate::core::summarize_context_history(&state.history);
                next.conversation = cache::extend_known_section(
                    self,
                    &source,
                    &previous.conversation,
                    &state.history,
                )?;
                next.transcript =
                    cache::append_section(self, &source, &previous.transcript, &suffix.transcript)?;
                next.context =
                    cache::write_section(self, &source, &state.model_context.snapshot())?;
                next.cursor = Some(cursor.clone());
                next.base = Some(AppTranscriptBase {
                    revision: previous.source.revision()?,
                    common_prefix: previous.transcript.rows,
                });
                cache::update_tools(self, &source, &mut next.tools, &suffix.transcript)?;
                cache::publish(self, &next)?;
                let (published, view, fresh) = cache::load_view(self)?;
                (fresh && published == source).then_some(view)
            };
            if let Some(view) = publish() {
                #[cfg(feature = "memory-diagnostics")]
                crate::memory_diagnostics::point(
                    "read_view.incremental.append_saved",
                    &[
                        ("conversation_appended", suffix.conversation_appended),
                        ("transcript_appended", suffix.transcript.len()),
                    ],
                );
                return Ok(view);
            }
        }
        drop(baseline);
        #[cfg(feature = "memory-diagnostics")]
        crate::memory_diagnostics::point("read_view.incremental.full_fallback", &[]);
        let transcript = full_renderer(&mut snapshot)?;
        let prefix = display_prefix(&mut snapshot, &state.history)?;
        let model = SessionReadModel {
            summary: cache::summary(self, &state.history)?,
            conversation: ConversationSnapshot {
                revision: state.history_revision,
                messages: state.history,
                model_context: state.model_context.snapshot(),
            },
            transcript,
            usage: state.usage,
            prefix: Some(prefix),
        };
        if cache::save_model(self, &source, &model, Some(&cursor)).is_some()
            && let Some((published, view, true)) = cache::load_view(self)
            && published == source
        {
            return Ok(view);
        }
        // A failed/displaced cache is never substituted for this validated view.
        Ok(SessionReadView {
            summary: model.summary,
            revision: model.conversation.revision,
            usage: model.usage,
            transcript: model.transcript,
            cursor: None,
        })
    }
}
