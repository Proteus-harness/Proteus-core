//! Context results are replay facts, never inferred from later model requests.
use std::collections::HashMap;

use anyhow::{Result, anyhow, ensure};

use crate::{
    core::{JournalEntry, JournalRecord},
    domain::{ExecutionId, ThreadId, TurnId},
};

use super::RecordedContextBuild;

pub(super) fn select(
    records: &[JournalRecord],
    execution: ExecutionId,
    thread: ThreadId,
    turn: TurnId,
) -> Result<Vec<RecordedContextBuild>> {
    let mut positions = HashMap::new();
    let mut requests = Vec::new();
    for record in records.iter().filter(|record| {
        record.execution_id == Some(execution)
            && record.thread_id == Some(thread)
            && record.turn_id == Some(turn)
    }) {
        match &record.entry {
            JournalEntry::ContextRequestRecorded(request) => {
                ensure!(
                    positions
                        .insert(request.invocation_id, requests.len())
                        .is_none(),
                    "context build {} has a duplicate request",
                    request.invocation_id
                );
                requests.push((request.clone(), None));
            }
            JournalEntry::ContextResponseRecorded(response) => {
                let index = positions.get(&response.invocation_id).ok_or_else(|| {
                    anyhow!(
                        "context build {} response has no selected request",
                        response.invocation_id
                    )
                })?;
                ensure!(
                    requests[*index].1.is_none(),
                    "context build {} has a duplicate outcome",
                    response.invocation_id
                );
                requests[*index].1 = Some(response.outcome.clone());
            }
            _ => {}
        }
    }
    requests
        .into_iter()
        .map(|(request, outcome)| {
            Ok(RecordedContextBuild {
                invocation_id: request.invocation_id,
                task: request.task,
                outcome: outcome.ok_or_else(|| {
                    anyhow!(
                        "context build {} is incomplete and cannot be used for workflow replay",
                        request.invocation_id
                    )
                })?,
            })
        })
        .collect()
}
