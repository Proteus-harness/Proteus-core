//! Cached display state is reconciled before its first execution, under the
//! existing OS writer lease. A read-only background refresh never takes it.
use crate::core::{JournalEntry, JournalProjection, model_context::ModelContextState};

use super::AgentRuntime;

impl AgentRuntime {
    pub(super) async fn validate_cached_resume(&self) -> anyhow::Result<()> {
        let mut deferred = self.session.deferred_resume_validation.lock().await;
        if !*deferred {
            return Ok(());
        }
        if let Some(store) = &self.session.session_store {
            let projection = store.prepare_resume_projection().await?;
            self.apply_resumed_projection(&projection).await;
        }
        *deferred = false;
        Ok(())
    }

    pub(crate) async fn refresh_cached_resume(&self) -> anyhow::Result<Option<JournalProjection>> {
        let deferred = self.session.deferred_resume_validation.lock().await;
        if !*deferred {
            return Ok(None);
        }
        let Some(store) = self.session.session_store.clone() else {
            return Ok(None);
        };
        let projection = tokio::task::spawn_blocking(move || store.load_projection()).await??;
        self.apply_resumed_projection(&projection).await;
        // A refreshed display is still not a writer-authorized continuation.
        Ok(Some(projection))
    }

    async fn apply_resumed_projection(&self, projection: &JournalProjection) {
        let context = ModelContextState::from_records(&projection.records, None);
        let mut history = self.session.history.lock().await;
        let mut model_context = self.session.model_context.lock().await;
        *history = projection.history.clone();
        *model_context = context;
        self.session.history_revision.store(
            projection.history_revision,
            std::sync::atomic::Ordering::SeqCst,
        );
        *self.session.history_turn.lock().await = projection
            .records
            .iter()
            .rev()
            .find_map(|record| {
                matches!(record.entry, JournalEntry::HistoryMutated(_)).then_some(record.turn_id)
            })
            .flatten();
        *self.session.interrupted_turns.lock().await = projection.interrupted_turns.clone();
        #[cfg(unix)]
        if let Some(store) = &self.session.session_store {
            let mut usage = crate::core::session_journal::UsageProjection::default();
            for record in &projection.records {
                usage.apply(record);
            }
            store.set_read_metadata(
                projection.history_revision,
                usage.snapshot(store.session_id()),
            );
        }
    }
}
