//! Persisted sessions retain their identity/lease while idle, not decoded history.
use std::sync::atomic::Ordering;

impl super::SessionState {
    pub(super) async fn release_history(&self) {
        let Some(store) = &self.session_store else {
            return;
        };
        // Serialize with admission and stale-view refresh. A newly reserved run
        // rechecks residency after acquiring the run lock.
        let _gate = self.deferred_resume_validation.lock().await;
        let mut history = self.history.lock().await;
        if self.history_resident.load(Ordering::SeqCst) {
            self.history_count.store(history.len(), Ordering::SeqCst);
        }
        *history = Vec::new();
        *self.model_context.lock().await = Default::default();
        self.history_resident.store(false, Ordering::SeqCst);
        store.hibernate_writer().await;
        drop(history);
        release_unused_pages();
    }
}

impl super::AgentRuntime {
    pub(super) async fn release_history_if_idle(&self) {
        if let Ok(_run) = self.session.run_lock.try_lock() {
            self.session.release_history().await;
        }
    }
}

fn release_unused_pages() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        // No live allocation is moved; this only returns free glibc pages.
        unsafe { malloc_trim(0) };
    }
}
