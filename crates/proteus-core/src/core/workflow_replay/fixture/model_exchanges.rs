use std::collections::HashMap;

use anyhow::{Result, anyhow};

use crate::{
    contracts::OperationOwner,
    core::{JournalEntry, JournalRecord, ModelResponseOutcome},
    domain::{ExchangeId, ExecutionId, ThreadId, TurnId},
    model_standard::CanonicalModelRequest,
};

use super::RecordedModelExchange;

struct PendingExchange {
    exchange_id: ExchangeId,
    owner: OperationOwner,
    request: CanonicalModelRequest,
    outcome: Option<ModelResponseOutcome>,
    completed_messages: Vec<crate::model_standard::CanonicalMessage>,
}

pub(super) fn select_exchanges(
    records: &[JournalRecord],
    execution_id: ExecutionId,
    thread_id: ThreadId,
    _turn_id: TurnId,
) -> Result<Vec<RecordedModelExchange>> {
    let mut exchanges = Vec::<PendingExchange>::new();
    let mut positions = HashMap::new();
    for record in records.iter().filter(|record| {
        record.execution_id == Some(execution_id) && record.thread_id == Some(thread_id)
    }) {
        match &record.entry {
            JournalEntry::ModelRequestRecorded(request) => {
                positions.insert(request.exchange_id, exchanges.len());
                exchanges.push(PendingExchange {
                    exchange_id: request.exchange_id,
                    owner: request.owner.clone(),
                    request: request.request.clone(),
                    outcome: None,
                    completed_messages: Vec::new(),
                });
            }
            JournalEntry::ModelMessageRecorded(item) => {
                let index = *positions
                    .get(&item.exchange_id)
                    .ok_or_else(|| anyhow!("completed model item has no root request"))?;
                exchanges[index]
                    .completed_messages
                    .push(item.message.clone());
            }
            JournalEntry::ModelResponseRecorded(response) => {
                let index = positions
                    .get(&response.exchange_id)
                    .copied()
                    .ok_or_else(|| {
                        anyhow!(
                            "model response {} has no selected root request",
                            response.exchange_id
                        )
                    })?;
                exchanges[index].outcome = Some(response.outcome.clone());
            }
            _ => {}
        }
    }
    exchanges
        .into_iter()
        .map(|exchange| {
            Ok(RecordedModelExchange {
                owner: exchange.owner,
                completed_messages: exchange.completed_messages,
                exchange_id: exchange.exchange_id,
                request: exchange.request,
                outcome: exchange.outcome.ok_or_else(|| {
                    anyhow!(
                        "model exchange {} is incomplete and cannot be used for workflow replay",
                        exchange.exchange_id
                    )
                })?,
            })
        })
        .collect()
}
