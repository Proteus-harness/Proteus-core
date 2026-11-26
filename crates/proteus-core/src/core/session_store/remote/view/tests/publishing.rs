//! A page cursor returned before the first cache write must already be usable.
use super::*;
use proteus_contracts::contracts::{
    SessionStorage, SessionStorageRequest as Request, SessionStorageResponse as Response,
};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

struct PausedCache {
    backend: Arc<Backend>,
    rejected: mpsc::Sender<()>,
    pause: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
}

#[async_trait::async_trait]
impl SessionStorage for PausedCache {
    async fn execute(&self, request: Request) -> anyhow::Result<Response> {
        self.execute_blocking(request)
    }

    fn execute_blocking(&self, request: Request) -> anyhow::Result<Response> {
        if matches!(request, Request::WriteCache { .. }) {
            let pause = self.pause.lock().unwrap().take();
            if let Some((entered, resume)) = pause {
                entered.send(())?;
                resume.recv_timeout(Duration::from_secs(15))?;
            }
        }
        let result = self.backend.execute_blocking(request)?;
        if matches!(&result, Response::CacheWritten { etag: None }) {
            let _ = self.rejected.send(());
        }
        Ok(result)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn immediate_scroll_and_pinned_history_survive_publication_or_concurrent_append() {
    use crate::core::PreparedSession;
    for advance in [false, true] {
        let (backend, original) = Backend::store();
        backend.append(2048);
        backend.append(2049);
        let path = original.session_dir().to_owned();
        let (entered, writing) = mpsc::channel();
        let (resume, paused) = mpsc::channel();
        let (rejected, failed) = mpsc::channel();
        let service = Arc::new(PausedCache {
            rejected,
            backend: backend.clone(),
            pause: Mutex::new(Some((entered, paused))),
        });
        let prepared = PreparedSession::from_dir(service.clone(), path.clone()).unwrap();
        let (live, view, _) = prepared.into_display_view(full).unwrap();
        let cursor = view.cursor.unwrap();
        assert_eq!(view.transcript.len(), 50);
        assert_eq!(cursor.before, 4047);
        writing.recv_timeout(Duration::from_secs(15)).unwrap();

        let reader = RemoteSessionStore::open(service, path).unwrap();
        if advance {
            backend.append(1);
        }
        assert_eq!(reader.saved_transcript_page(None, None).is_some(), !advance);
        assert_eq!(
            reader.fresh_context_summary().map(|s| s.messages),
            (!advance).then_some(4097)
        );
        assert_eq!(reader.fresh_read_usage().is_some(), !advance);
        let tool = reader.saved_tool_state("plan");
        assert_eq!(
            tool.as_ref().map(|t| &t.revision),
            (!advance).then_some(&cursor.revision)
        );
        assert!(tool.and_then(|t| t.tool).is_none());
        assert_eq!(
            reader.saved_display_page().unwrap().revision,
            cursor.revision
        );
        let rows: Vec<_> = backend
            .0
            .lock()
            .unwrap()
            .records
            .iter()
            .take(2)
            .flat_map(|record| {
                let proteus_contracts::storage::journal::JournalEntry::HistoryMutated(change) =
                    &record.entry
                else {
                    unreachable!()
                };
                change
                    .messages
                    .iter()
                    .map(|msg| super::support::row(msg, record.timestamp_ms))
            })
            .collect();
        let wire = |rows: &[AppTranscriptMessage]| serde_json::to_value(rows).unwrap();
        assert_eq!(
            wire(&reader.saved_transcript(&cursor.revision).unwrap()),
            wire(&rows)
        );
        let mut before = cursor.before;
        while before != 0 {
            let page = reader
                .saved_transcript_page(Some(before), Some(&cursor.revision))
                .unwrap();
            assert_eq!(page.total, rows.len());
            assert_eq!(wire(&page.messages), wire(&rows[page.start..before]));
            before = page.start;
        }
        assert_eq!(wire(&view.transcript), wire(&rows[cursor.before..]));
        assert!(
            reader
                .saved_transcript_page(Some(rows.len() + 1), Some(&cursor.revision))
                .is_none()
        );
        resume.send(()).unwrap();
        if advance {
            failed.recv_timeout(Duration::from_secs(15)).unwrap();
        } else {
            tokio::time::timeout(Duration::from_secs(15), async {
                while live.publishing_transcript(Some(&cursor.revision)).is_some() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        }
        let first = reader
            .saved_transcript_page(Some(2), Some(&cursor.revision))
            .unwrap();
        assert_eq!(wire(&first.messages), wire(&rows[..2]));
    }
}
