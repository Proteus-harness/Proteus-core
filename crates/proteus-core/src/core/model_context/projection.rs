//! Incremental context facts from committed journal records; no message payloads retained.
use std::collections::HashMap;

use super::ModelContextState;
use crate::{
    contracts::ModelContextObservation,
    core::{HistoryMutationKind, JournalEntry, JournalRecord, ModelResponseOutcome},
    domain::{ExecutionId, ThreadId, TurnId},
};

#[derive(Debug, Default)]
pub(crate) struct ModelContextProjection {
    pub(super) context: ModelContextState,
    root_executions: HashMap<TurnId, (ThreadId, ExecutionId)>,
    before_latest_turn: Option<(TurnId, Vec<ModelContextObservation>)>,
}

impl ModelContextProjection {
    pub(crate) fn snapshot(&self) -> Vec<ModelContextObservation> {
        self.context.snapshot()
    }

    pub(crate) fn before_turn(&self, turn: TurnId) -> Option<Vec<ModelContextObservation>> {
        self.before_latest_turn
            .as_ref()
            .filter(|(id, _)| *id == turn)
            .map(|(_, observations)| observations.clone())
    }

    /// Called only after journal validation and durable commit, or during replay.
    pub(crate) fn apply(&mut self, record: &JournalRecord) {
        match &record.entry {
            JournalEntry::TurnOpened(_) => {
                if let Some(turn) = record.turn_id {
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
