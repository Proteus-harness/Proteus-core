//! Policy-free context accounting facts retained across workflow invocations.
//!
//! The workflow interprets window observations. The host never turns an error
//! into provider usage or chooses a compaction threshold.
use std::collections::HashMap;

use crate::{
    contracts::{ModelCallOrigin, ModelContextObservation},
    core::{JournalEntry, JournalRecord},
    domain::{ExchangeId, TurnId},
    model_standard::{
        CanonicalModelRequest, CanonicalModelResponse, ModelFailure, ModelFailureKind,
    },
};

pub(crate) mod projection;
pub(crate) mod recorder;

#[derive(Debug, Default)]
pub(crate) struct ModelContextState {
    observations: Vec<ModelContextObservation>,
    requests: HashMap<ExchangeId, (ModelCallOrigin, Option<u32>)>,
}

impl ModelContextState {
    pub(crate) fn from_snapshot(observations: Vec<ModelContextObservation>) -> Self {
        Self {
            observations,
            requests: HashMap::new(),
        }
    }

    pub(crate) fn snapshot(&self) -> Vec<ModelContextObservation> {
        self.observations.clone()
    }

    pub(crate) fn request(
        &mut self,
        exchange: ExchangeId,
        origin: ModelCallOrigin,
        request: &CanonicalModelRequest,
    ) {
        self.requests
            .insert(exchange, (origin, request.limits.max_input_tokens));
    }

    pub(crate) fn response(&mut self, exchange: ExchangeId, response: &CanonicalModelResponse) {
        if self.requests.remove(&exchange).is_none() {
            return;
        }
        let Some(usage) = &response.usage else { return };
        let tokens = u64::from(usage.input_tokens) + u64::from(usage.output_tokens);
        let last = tokens.min(u64::from(u32::MAX)) as u32;
        if let Some(ModelContextObservation::Usage {
            total_tokens,
            last_tokens,
        }) = self.observations.last_mut()
        {
            *total_tokens = total_tokens.saturating_add(tokens);
            *last_tokens = last;
        } else {
            self.observations.push(ModelContextObservation::Usage {
                total_tokens: tokens,
                last_tokens: last,
            });
        }
    }

    pub(crate) fn failure(&mut self, exchange: ExchangeId, failure: &ModelFailure) {
        let Some((ModelCallOrigin::Direct, Some(max_input_tokens))) =
            self.requests.remove(&exchange)
        else {
            return;
        };
        if failure.kind == ModelFailureKind::ContextWindowExceeded {
            self.observations
                .push(ModelContextObservation::ContextWindowExceeded { max_input_tokens });
        }
    }

    pub(crate) fn compacted(&mut self) {
        if !matches!(
            self.observations.last(),
            Some(ModelContextObservation::HistoryCompacted)
        ) {
            self.observations
                .push(ModelContextObservation::HistoryCompacted);
        }
    }

    /// Build the same facts for cold resume and for the prefix preceding a
    /// replayed turn. Root TurnOpened lineage survives resume with a new thread;
    /// detached and child executions cannot affect the shared conversation.
    pub(crate) fn from_records(records: &[JournalRecord], before_turn: Option<TurnId>) -> Self {
        let mut projection = projection::ModelContextProjection::default();
        for record in records {
            if matches!(record.entry, JournalEntry::TurnOpened(_))
                && before_turn.is_some_and(|turn| record.turn_id == Some(turn))
            {
                break;
            }
            projection.apply(record);
        }
        let mut state = projection.context;
        // Incomplete exchanges from a dead process cannot complete in this one.
        state.requests.clear();
        state
    }
}

#[cfg(test)]
mod tests;
