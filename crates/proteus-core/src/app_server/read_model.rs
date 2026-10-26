//! Stale display state refreshes independently, without acquiring write ownership.
use super::{AppServerEvent, AppServerHandle, journal_transcript_messages};
use crate::core::{SessionReadModel, SessionStore};

use proteus_contracts::app_protocol::{AppTranscriptCursor, AppTranscriptMessage};

#[derive(Default)]
pub(super) struct CompletedTranscript {
    pub messages: Vec<AppTranscriptMessage>,
    pub cursor: Option<AppTranscriptCursor>,
}

impl From<Vec<AppTranscriptMessage>> for CompletedTranscript {
    fn from(messages: Vec<AppTranscriptMessage>) -> Self {
        Self {
            messages,
            cursor: None,
        }
    }
}

impl CompletedTranscript {
    pub fn from_model(
        store: &SessionStore,
        transcript: Vec<AppTranscriptMessage>,
        stale: bool,
    ) -> Self {
        #[cfg(unix)]
        if let Some(page) = if stale {
            store.saved_display_page()
        } else {
            store.saved_transcript_page(None, None)
        } {
            return Self {
                messages: page.messages,
                cursor: Some(AppTranscriptCursor {
                    revision: page.revision,
                    before: page.start,
                    base: page.base,
                }),
            };
        }
        let _ = (store, stale);
        transcript.into()
    }
}

/// Cold HTTP reads share the same current projection without opening a runtime.
pub(super) fn current_read_model(store: &SessionStore) -> anyhow::Result<SessionReadModel> {
    match store.fresh_read_model() {
        Some(model) => Ok(model),
        None => {
            store.project_read_model(|projection| journal_transcript_messages(projection, None))
        }
    }
}

pub(super) fn project_completed_transcript(
    store: &SessionStore,
) -> anyhow::Result<CompletedTranscript> {
    // Drop replay records, JSON trees and the full cached conversation before
    // returning free pages. The completed page and writer's live state survive.
    let result = (|| {
        let model =
            store.project_read_model(|projection| journal_transcript_messages(projection, None))?;
        Ok(CompletedTranscript::from_model(
            store,
            model.transcript,
            false,
        ))
    })();
    release_projection_memory();
    result
}

fn release_projection_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        // SAFETY: glibc's thread-safe hint releases only unused allocator pages;
        // it takes no pointers and cannot move or invalidate live allocations.
        // This runs once per completed blocking projection, never per event.
        unsafe { malloc_trim(0) };
    }
}

pub(super) fn refresh_in_background(server: &AppServerHandle) {
    let server = server.clone();
    let Some(baseline) = server.events.completed_history() else {
        return;
    };
    tokio::spawn(async move {
        let result: anyhow::Result<()> = async {
            let Some(dir) = server.runtime.session_dir().map(std::path::Path::to_owned) else {
                return Ok(());
            };
            #[cfg(unix)]
            let source = {
                let dir = dir.clone();
                tokio::task::spawn_blocking(move || {
                    crate::core::session_read_cache::SourceFingerprint::read(&dir, 8192)
                })
                .await?
            };
            let Some(projection) = server.runtime.refresh_cached_resume().await? else {
                return Ok(());
            };
            let transcript = tokio::task::spawn_blocking(move || {
                let store = SessionStore::open(dir)?;
                let transcript = journal_transcript_messages(&projection, None);
                let model = SessionReadModel::from_projection(&store, projection, transcript)?;
                #[cfg(unix)]
                if let Some(source) = source {
                    store.save_read_model(&source, &model);
                }
                Ok::<_, anyhow::Error>(CompletedTranscript::from_model(
                    &store,
                    model.transcript,
                    false,
                ))
            })
            .await??;
            server.events.refresh_history(&baseline, transcript)?;
            Ok(())
        }
        .await;
        if let Err(error) = result {
            let message = format!("Не удалось обновить сохранённую историю: {error:#}");
            eprintln!("warning: {message}");
            let _ = server.events.send(AppServerEvent::Error { message });
        }
    });
}
