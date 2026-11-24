//! Incremental context facts from committed journal records; no message payloads retained.
use std::collections::HashMap;

use super::ModelContextState;
use crate::{
    contracts::ModelContextObservation,
    domain::{ExecutionId, ThreadId, TurnId},
    journal::{HistoryMutationKind, JournalEntry, JournalRecord, ModelResponseOutcome},
};

#[derive(Debug, Default)]
pub struct ModelContextProjection {
    pub(super) context: ModelContextState,
    root_executions: HashMap<TurnId, (ThreadId, ExecutionId)>,
    before_latest_turn: Option<(TurnId, Vec<ModelContextObservation>)>,
    omit_turn_snapshots: bool,
}

impl ModelContextProjection {
    /// Project the current facts without retaining the prior state of each turn.
    /// Use only when the caller never queries before_turn; lineage filtering
    /// and the current observations are the same as in default mode.
    pub fn current_only() -> Self {
        Self {
            omit_turn_snapshots: true,
            ..Self::default()
        }
    }

    pub fn snapshot(&self) -> Vec<ModelContextObservation> {
        self.context.snapshot()
    }

    pub fn before_turn(&self, turn: TurnId) -> Option<Vec<ModelContextObservation>> {
        self.before_latest_turn
            .as_ref()
            .filter(|(id, _)| *id == turn)
            .map(|(_, observations)| observations.clone())
    }

    /// Called only after journal validation and durable commit, or during replay.
    pub fn apply(&mut self, record: &JournalRecord) {
        match &record.entry {
            JournalEntry::TurnOpened(_) => {
                if !self.omit_turn_snapshots
                    && let Some(turn) = record.turn_id
                {
                    self.before_latest_turn = Some((turn, self.context.snapshot()));
                }
                if let (Some(turn), Some(thread), Some(execution)) =
                    (record.turn_id, record.thread_id, record.execution_id)
                {
                    self.root_executions.insert(turn, (thread, execution));
                }
            }
            JournalEntry::HistoryMutated(mutation) => {
                if mutation
                    .compaction
                    .as_ref()
                    .is_some_and(|report| report.changed)
                {
                    self.context.compacted();
                } else if mutation.mutation == HistoryMutationKind::Replace {
                    self.context = ModelContextState::default();
                }
            }
            JournalEntry::ModelRequestRecorded(request)
                if record
                    .turn_id
                    .and_then(|turn| self.root_executions.get(&turn).copied())
                    .is_some_and(|owner| {
                        Some(owner) == record.thread_id.zip(record.execution_id)
                    }) =>
            {
                self.context
                    .request(request.exchange_id, request.origin, &request.request);
            }
            JournalEntry::ModelResponseRecorded(response)
                if record
                    .turn_id
                    .and_then(|turn| self.root_executions.get(&turn).copied())
                    .is_some_and(|owner| {
                        Some(owner) == record.thread_id.zip(record.execution_id)
                    }) =>
            {
                match &response.outcome {
                    ModelResponseOutcome::Response { response: model } => {
                        self.context.response(response.exchange_id, model);
                    }
                    ModelResponseOutcome::Error { failure } => {
                        self.context.failure(response.exchange_id, failure);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{
            AgentTask, new_execution_id, new_record_id, new_session_id, new_thread_id, new_turn_id,
        },
        journal::{JOURNAL_SCHEMA_VERSION, TurnOpened},
    };

    fn opened(turn: TurnId) -> JournalRecord {
        JournalRecord {
            schema_version: JOURNAL_SCHEMA_VERSION,
            record_id: new_record_id(),
            session_seq: 1,
            timestamp_ms: 0,
            session_id: new_session_id(),
            execution_id: Some(new_execution_id()),
            thread_id: Some(new_thread_id()),
            turn_id: Some(turn),
            entry: JournalEntry::TurnOpened(TurnOpened {
                task: AgentTask::new("fixture", "/tmp".into()),
                intent: None,
                base_history_revision: 0,
                module_epoch: 0,
                config_snapshot: serde_json::json!({}),
            }),
        }
    }

    #[test]
    fn current_only_keeps_lineage_and_current_facts_but_not_turn_snapshots() {
        let turn = new_turn_id();
        let opening = opened(turn);
        let mut ordinary = ModelContextProjection::default();
        let mut current = ModelContextProjection::current_only();
        for projection in [&mut ordinary, &mut current] {
            projection.context.compacted();
            projection.apply(&opening);
        }
        assert_eq!(current.root_executions, ordinary.root_executions);
        assert_eq!(
            ordinary.before_turn(turn),
            Some(vec![ModelContextObservation::HistoryCompacted])
        );
        assert_eq!(current.before_turn(turn), None);
        for projection in [&mut ordinary, &mut current] {
            projection
                .context
                .observations
                .push(ModelContextObservation::Usage {
                    total_tokens: 12,
                    last_tokens: 12,
                });
        }
        assert_eq!(current.snapshot(), ordinary.snapshot());
        assert_eq!(
            ordinary.before_turn(turn),
            Some(vec![ModelContextObservation::HistoryCompacted])
        );
    }
}
