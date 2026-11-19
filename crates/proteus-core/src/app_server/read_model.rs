//! Stale display state refreshes independently, without acquiring write ownership.
use super::{
    AppServerEvent, AppServerHandle, journal_transcript_append_from_source,
    journal_transcript_from_source,
};
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

/// Read a complete named display revision. A disposable cache miss must not
/// make the canonical history unreadable, but a newer source is not that revision.
#[cfg(unix)]
pub(super) fn transcript_at_revision(
    store: &SessionStore,
    revision: &str,
) -> anyhow::Result<Vec<AppTranscriptMessage>> {
    if let Some(messages) = store.saved_transcript(revision) {
        return Ok(messages);
    }
    Ok(rebuild_transcript(store, Some(revision))?.1)
}

/// Read a bounded page, repairing a missing/corrupt index at most once.
#[cfg(unix)]
pub(super) fn transcript_page(
    store: &SessionStore,
    before: Option<usize>,
    expected: Option<&str>,
) -> anyhow::Result<proteus_contracts::app_protocol::AppTranscriptPage> {
    if let Some(page) = store.saved_transcript_page(before, expected) {
        return Ok(page);
    }
    let (revision, messages) = rebuild_transcript(store, expected)?;
    let total = messages.len();
    let end = before.unwrap_or(total);
    anyhow::ensure!(
        end <= total,
        "history position is outside the requested revision"
    );
    let start = end.saturating_sub(50);
    Ok(proteus_contracts::app_protocol::AppTranscriptPage {
        revision,
        start,
        total,
        messages: messages.into_iter().skip(start).take(end - start).collect(),
        // Rebuilding cannot prove a relationship to a lost display projection.
        base: None,
    })
}

#[cfg(unix)]
fn rebuild_transcript(
    store: &SessionStore,
    expected: Option<&str>,
) -> anyhow::Result<(String, Vec<AppTranscriptMessage>)> {
    use crate::core::session_read_cache::SourceFingerprint;

    // Reject an unavailable named revision before rebuilding anything. An
    // unnamed cold page needs no preliminary OpenSnapshot: its strict display
    // read already captures the input fingerprint (also on canonical fallback).
    let named = if let Some(expected) = expected {
        let source = SourceFingerprint::try_read(store)?;
        anyhow::ensure!(
            source.revision().as_deref() == Some(expected),
            "history revision changed; refresh the session snapshot"
        );
        Some(source)
    } else {
        None
    };
    let (source, model) = store.project_sourced_read_model(named.as_ref(), |snapshot| {
        journal_transcript_from_source(snapshot, None)
    })?;
    anyhow::ensure!(
        SourceFingerprint::try_read(store)? == source,
        "history source changed during recovery; refresh the session snapshot"
    );
    let revision = source
        .revision()
        .ok_or_else(|| anyhow::anyhow!("history source revision is unavailable"))?;
    // Cache writes are best effort (e.g. disk full). The verified in-memory
    // result remains usable without retrying indefinitely or requiring a writer lease.
    Ok((revision, model.transcript))
}

/// Cold HTTP reads share the same current projection without opening a runtime.
pub(super) fn current_read_model(store: &SessionStore) -> anyhow::Result<SessionReadModel> {
    match store.fresh_read_model() {
        Some(model) => Ok(model),
        None => store.project_read_model(|source| journal_transcript_from_source(source, None)),
    }
}

pub(super) fn project_committed_transcript(
    store: &SessionStore,
    committed: Option<crate::core::CommittedDisplay>,
) -> anyhow::Result<CompletedTranscript> {
    if let Some(committed) = committed
        && let Some(view) = store.publish_committed_view(
            committed,
            super::transcript::journal_transcript_append_committed,
        )
    {
        release_projection_memory();
        return Ok(CompletedTranscript {
            messages: view.transcript,
            cursor: view.cursor,
        });
    }
    project_completed_transcript(store)
}

pub(super) fn project_completed_transcript(
    store: &SessionStore,
) -> anyhow::Result<CompletedTranscript> {
    // The incremental path retains only the new display rows and the bounded
    // completed page. Uncertain dependencies use the full verified projector.
    let result = (|| {
        let view = store.project_read_view(
            |source| journal_transcript_from_source(source, None),
            journal_transcript_append_from_source,
        )?;
        Ok(CompletedTranscript {
            messages: view.transcript,
            cursor: view.cursor,
        })
    })();
    #[cfg(feature = "memory-diagnostics")]
    crate::memory_diagnostics::point("completed.full_model_dropped", &[]);
    release_projection_memory();
    #[cfg(feature = "memory-diagnostics")]
    crate::memory_diagnostics::point("completed.after_trim", &[]);
    result
}

pub(super) fn release_projection_memory() {
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

pub(super) fn refresh_in_background(
    server: &AppServerHandle,
    cached_source: crate::core::session_read_cache::SourceFingerprint,
) {
    let server = server.clone();
    let Some(baseline) = server.events.completed_history() else {
        return;
    };
    tokio::spawn(async move {
        let result: anyhow::Result<()> = async {
            let Some(store) = server.runtime.session_store().cloned() else {
                return Ok(());
            };
            let Some(read) = server.runtime.refresh_cached_resume(cached_source).await? else {
                return Ok(());
            };
            let transcript = tokio::task::spawn_blocking(move || {
                let view = store.project_validated_view(read, cfg!(unix), |source| {
                    journal_transcript_from_source(source, None)
                })?;
                Ok::<_, anyhow::Error>(CompletedTranscript {
                    messages: view.transcript,
                    cursor: view.cursor,
                })
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

#[cfg(all(test, unix))]
mod tests;

/// A current validated display must exist before compact tool state is readable.
#[derive(Debug)]
pub(super) struct ToolStateNotReady;

impl std::fmt::Display for ToolStateNotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("tool state is not ready; refresh the session history snapshot before retrying")
    }
}

impl std::error::Error for ToolStateNotReady {}

#[cfg(unix)]
pub(super) fn latest_tool_state(
    store: &SessionStore,
    name: &str,
) -> anyhow::Result<proteus_contracts::app_protocol::AppLatestToolState> {
    store
        .saved_tool_state(name)
        .ok_or_else(|| ToolStateNotReady.into())
}

#[cfg(test)]
pub(crate) mod faults;
