//! Cached display state is reconciled before its first execution, under the
//! existing OS writer lease. A read-only background refresh never takes it.
use crate::core::{
    model_context::ModelContextState,
    session_journal::{JournalResumeState, ValidatedJournalRead},
};

use super::AgentRuntime;

impl AgentRuntime {
    #[cfg(test)]
    pub(crate) async fn lock_resume_validation_for_test(
        &self,
    ) -> tokio::sync::MutexGuard<'_, bool> {
        self.session.deferred_resume_validation.lock().await
    }

    pub(super) async fn validate_cached_resume(&self) -> anyhow::Result<()> {
        let mut deferred = self.session.deferred_resume_validation.lock().await;
        if !*deferred
            && self
                .session
                .history_resident
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(());
        }
        if let Some(store) = &self.session.session_store {
            let state = store.prepare_resume_state().await?;
            self.apply_resumed_state(state).await;
        }
        *deferred = false;
        Ok(())
    }

    pub(crate) async fn refresh_cached_resume(
        &self,
    ) -> anyhow::Result<Option<ValidatedJournalRead>> {
        let deferred = self.session.deferred_resume_validation.lock().await;
        if !*deferred {
            return Ok(None);
        }
        let Some(store) = self.session.session_store.clone() else {
            return Ok(None);
        };
        let read = tokio::task::spawn_blocking(move || {
            ValidatedJournalRead::open(store.session_dir(), store.session_id())
        })
        .await??;
        let mut history = self.session.history.lock().await;
        let mut model_context = self.session.model_context.lock().await;
        self.session
            .history_count
            .store(read.history().len(), std::sync::atomic::Ordering::SeqCst);
        if self
            .session
            .history_resident
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            *history = read.history().to_vec();
            *model_context = ModelContextState::from_snapshot(read.model_context());
        }
        self.session
            .history_revision
            .store(read.history_revision(), std::sync::atomic::Ordering::SeqCst);
        *self.session.history_turn.lock().await = read.last_history_turn();
        *self.session.interrupted_turns.lock().await = read.interrupted_turns();
        #[cfg(unix)]
        if let Some(store) = &self.session.session_store {
            store.set_read_metadata(read.history_revision(), read.usage());
        }
        // A refreshed display is still not a writer-authorized continuation.
        Ok(Some(read))
    }

    async fn apply_resumed_state(&self, state: JournalResumeState) {
        let mut history = self.session.history.lock().await;
        let mut model_context = self.session.model_context.lock().await;
        self.session
            .history_count
            .store(state.history.len(), std::sync::atomic::Ordering::SeqCst);
        // The writer owns validation history; this is the sole additional owned
        // history for execution. No raw projection or third copy is created.
        *history = state.history;
        *model_context = state.model_context;
        self.session
            .history_resident
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.session
            .history_revision
            .store(state.history_revision, std::sync::atomic::Ordering::SeqCst);
        *self.session.history_turn.lock().await = state.last_history_turn;
        *self.session.interrupted_turns.lock().await = state.interrupted_turns;
        #[cfg(unix)]
        if let Some(store) = &self.session.session_store {
            store.set_read_metadata(state.history_revision, state.usage);
        }
        #[cfg(feature = "memory-diagnostics")]
        crate::memory_diagnostics::point(
            "resume.compact_state_moved",
            &[("history", history.len())],
        );
    }
}
