//! Bounded background work for stale catalog entries, including unopened sessions.
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, OnceLock},
};

use tokio::sync::watch;

const WORKERS: usize = 2;
const MAX_PENDING: usize = 128;

#[derive(Default)]
struct QueueState {
    pending: VecDeque<PathBuf>,
    outstanding: HashSet<PathBuf>,
}
impl QueueState {
    fn enqueue(&mut self, dir: PathBuf) -> bool {
        if self.pending.len() >= MAX_PENDING || !self.outstanding.insert(dir.clone()) {
            return false;
        }
        self.pending.push_back(dir);
        true
    }
    fn take(&mut self) -> Option<PathBuf> {
        self.pending.pop_front()
    }
    fn finish(&mut self, dir: &std::path::Path) {
        self.outstanding.remove(dir);
    }
}

#[derive(Default)]
struct Queue {
    state: Mutex<QueueState>,
    available: Condvar,
}

fn updates() -> &'static watch::Sender<u64> {
    static UPDATES: OnceLock<watch::Sender<u64>> = OnceLock::new();
    UPDATES.get_or_init(|| watch::channel(0).0)
}
pub(super) fn subscribe_updates() -> watch::Receiver<u64> {
    updates().subscribe()
}

pub(super) fn enqueue(dir: PathBuf) {
    static QUEUE: OnceLock<Arc<Queue>> = OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        let queue = Arc::new(Queue::default());
        for index in 0..WORKERS {
            let worker_queue = Arc::clone(&queue);
            // A spawn failure only loses best-effort acceleration; startup still
            // returns the saved view, and the queue remains strictly bounded.
            let _ = std::thread::Builder::new()
                .name(format!("session-summary-{index}"))
                .spawn(move || worker(worker_queue));
        }
        queue
    });
    let added = queue
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .enqueue(dir);
    if added {
        queue.available.notify_one();
    }
}

fn worker(queue: Arc<Queue>) {
    loop {
        let dir = {
            let mut state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(dir) = state.take() {
                    break dir;
                }
                state = queue
                    .available
                    .wait(state)
                    .unwrap_or_else(|e| e.into_inner());
            }
        };
        let saved = refresh_one(&dir);
        queue
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .finish(&dir);
        if saved {
            updates().send_modify(|generation| *generation = generation.wrapping_add(1));
        }
    }
}

fn refresh_one(dir: &std::path::Path) -> bool {
    let before = super::SourceFingerprint::read(dir, super::MAX_DISK_BLOBS);
    let Ok(summary) = super::super::session_summary_from_dir(dir.to_owned()) else {
        return false;
    };
    super::cache_if_unchanged(super::shared_cache(), dir, before, summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_deduplicates_pending_and_running_work_and_bounds_waiting_work() {
        let mut state = QueueState::default();
        let first = PathBuf::from("first");
        assert!(state.enqueue(first.clone()));
        assert!(!state.enqueue(first.clone()));
        assert_eq!(state.take(), Some(first.clone()));
        assert!(
            !state.enqueue(first.clone()),
            "running jobs must remain deduplicated"
        );
        for index in 0..MAX_PENDING {
            assert!(state.enqueue(PathBuf::from(index.to_string())));
        }
        assert!(!state.enqueue(PathBuf::from("overflow")));
        assert_eq!(state.pending.len(), MAX_PENDING);
        assert_eq!(state.outstanding.len(), MAX_PENDING + 1);
        state.finish(&first);
        state.take();
        assert!(state.enqueue(first));
    }

    #[tokio::test]
    async fn failed_background_refresh_preserves_stale_cache_and_canonical_files() {
        use crate::{
            core::SessionStore,
            domain::{new_session_id, new_thread_id},
            model_standard::{CanonicalMessage, MessageRole},
        };
        let root = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path(), workspace.path(), new_session_id()).unwrap();
        store
            .append_history(
                new_thread_id(),
                None,
                &[CanonicalMessage::text(MessageRole::User, "synthetic")],
            )
            .await
            .unwrap();
        assert!(refresh_one(store.session_dir()));
        let cache_path = store.session_dir().join(".read-summary.sqlite");
        let old_cache = std::fs::read(&cache_path).unwrap();
        std::fs::write(store.journal_path(), b"{broken}\n").unwrap();
        let journal = std::fs::read(store.journal_path()).unwrap();
        assert!(!refresh_one(store.session_dir()));
        assert_eq!(std::fs::read(&cache_path).unwrap(), old_cache);
        assert_eq!(std::fs::read(store.journal_path()).unwrap(), journal);
        let (_, summary, fresh) = super::super::read_cache::load_any::<
            proteus_contracts::app_protocol::AppSessionSummary,
        >(store.session_dir(), super::super::DISK_NAME)
        .unwrap();
        assert!(!fresh);
        assert_eq!(summary.message_count, 1);
    }
}
