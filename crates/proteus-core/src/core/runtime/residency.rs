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
        #[cfg(feature = "memory-diagnostics")]
        crate::memory_diagnostics::point(
            "release.before_history_drop",
            &[("history", history.len())],
        );
        *history = Vec::new();
        #[cfg(feature = "memory-diagnostics")]
        crate::memory_diagnostics::point("release.after_history_drop", &[]);
        *self.model_context.lock().await = Default::default();
        #[cfg(feature = "memory-diagnostics")]
        crate::memory_diagnostics::point("release.after_model_context_drop", &[]);
        self.history_resident.store(false, Ordering::SeqCst);
        store.hibernate_writer().await;
        #[cfg(feature = "memory-diagnostics")]
        crate::memory_diagnostics::point("release.after_writer_hibernate", &[]);
        drop(history);
        #[cfg(feature = "memory-diagnostics")]
        crate::memory_diagnostics::point("release.before_trim", &[]);
        crate::release_unused_allocator_pages();
        #[cfg(feature = "memory-diagnostics")]
        crate::memory_diagnostics::point("release.after_trim", &[]);
    }
}

impl super::AgentRuntime {
    pub(super) async fn release_history_if_idle(&self) {
        if let Ok(_run) = self.session.run_lock.try_lock() {
            self.session.release_history().await;
        }
    }

    pub(crate) async fn release_idle_component_workers(&self) -> anyhow::Result<usize> {
        let Ok(_run) = self.session.run_lock.try_lock() else {
            return Ok(0);
        };
        let state = self.services.execution_state.read().await;
        state
            .runtime
            .registry
            .release_idle_component_workers()
            .await
    }
}
