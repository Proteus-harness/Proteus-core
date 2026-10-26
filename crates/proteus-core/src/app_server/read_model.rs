//! Stale display state refreshes independently, without acquiring write ownership.
use super::{AppServerEvent, AppServerHandle, journal_transcript_messages};
use crate::core::{SessionReadModel, SessionStore};

/// Cold HTTP reads share the same current projection without opening a runtime.
pub(super) fn current_read_model(store: &SessionStore) -> anyhow::Result<SessionReadModel> {
    match store.fresh_read_model() {
        Some(model) => Ok(model),
        None => {
            store.project_read_model(|projection| journal_transcript_messages(projection, None))
        }
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
                Ok::<_, anyhow::Error>(model.transcript)
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
