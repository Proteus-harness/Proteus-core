//! Incremental display projection never substitutes a cache for strict validation.
use super::*;
use crate::core::session_journal::JournalReadCursor;

impl SessionStore {
    pub(crate) fn project_read_view(
        &self,
        mut full_renderer: impl FnMut(
            &mut JournalReadSnapshot,
        ) -> anyhow::Result<Vec<AppTranscriptMessage>>,
        mut append_renderer: impl FnMut(
            &mut JournalReadSnapshot,
            &JournalReadCursor,
            &[[u8; 32]],
            usize,
        ) -> anyhow::Result<Option<SessionTranscriptAppend>>,
    ) -> anyhow::Result<SessionReadView> {
        #[cfg(feature = "memory-diagnostics")]
        let memory = crate::memory_diagnostics::Scope::new("read_view.project");
        #[cfg(not(unix))]
        {
            let _ = &mut append_renderer;
            let model = self.project_read_model(full_renderer)?;
            return Ok(SessionReadView::from_model(self, model, false));
        }
        #[cfg(unix)]
        {
            use super::super::read_cache::SourceFingerprint;

            let previous = sqlite::checkpoint(self);
            let source = SourceFingerprint::read(self.session_dir(), 8192);
            // Even a matching checkpoint cannot authorize skipping old blobs
            // or canonical semantic validation. Only display work is reduced.
            let (mut read, prefix_matches) = ValidatedJournalRead::open_with_cursor(
                self.session_dir(),
                self.session_id(),
                previous.as_ref().map(|previous| previous.cursor()),
            )?;
            let cursor = read.cursor();
            let baseline = read.take_prefix_history_fingerprints();
            let (mut snapshot, state) = read.into_parts();
            if let (true, Some(previous), Some(source), Some(baseline)) = (
                prefix_matches,
                previous.as_ref(),
                source.as_ref(),
                baseline.as_deref(),
            ) && baseline.len() == previous.conversation_len()
                && let Some(suffix) = append_renderer(
                    &mut snapshot,
                    previous.cursor(),
                    baseline,
                    state.history.len(),
                )?
                && state.history.len().checked_sub(previous.conversation_len())
                    == Some(suffix.conversation_appended)
            {
                let summary = super::super::catalog::session_summary_from_history(
                    self.session_dir().to_owned(),
                    self.session_id(),
                    &state.history,
                )?;
                if sqlite::append(self, source, previous, &cursor, &state, &summary, &suffix)
                    .is_some()
                    && let Some(view) = view_for_source(self, source)
                {
                    super::super::read_cache::save(self.session_dir(), "summary", source, &summary);
                    #[cfg(feature = "memory-diagnostics")]
                    memory.sample(
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
            memory.sample("read_view.incremental.full_fallback", &[]);
            // Cache misses, uncertain suffixes, contention and source races use
            // the full renderer on the already strictly validated pinned read.
            let transcript = full_renderer(&mut snapshot)?;
            let summary = super::super::catalog::session_summary_from_history(
                self.session_dir().to_owned(),
                self.session_id(),
                &state.history,
            )?;
            let model = SessionReadModel {
                summary,
                conversation: ConversationSnapshot {
                    revision: state.history_revision,
                    messages: state.history,
                    model_context: state.model_context.snapshot(),
                },
                transcript,
                usage: state.usage,
            };
            if let Some(source) = source
                && sqlite::save_with_cursor(self, &source, &model, &cursor)
            {
                super::super::read_cache::save(
                    self.session_dir(),
                    "summary",
                    &source,
                    &model.summary,
                );
                if let Some(view) = view_for_source(self, &source) {
                    return Ok(view);
                }
            }
            // Failed cache publication must never replace a verified result
            // with another cached generation, even when the source raced.
            Ok(SessionReadView {
                summary: model.summary,
                revision: model.conversation.revision,
                usage: model.usage,
                transcript: model.transcript,
                cursor: None,
            })
        }
    }
}

#[cfg(unix)]
fn view_for_source(
    store: &SessionStore,
    expected: &super::super::read_cache::SourceFingerprint,
) -> Option<SessionReadView> {
    let (source, view, fresh) = sqlite::view(store)?;
    (fresh && &source == expected).then_some(view)
}
