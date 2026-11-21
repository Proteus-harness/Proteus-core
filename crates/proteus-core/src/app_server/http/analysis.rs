//! Historical inspection reads storage without admitting agent execution.
use std::{collections::HashMap, path::PathBuf};

use anyhow::{Result, anyhow};
use proteus_contracts::{app_protocol::analysis::*, domain::TurnId};

use super::{
    HttpAppState,
    sessions::{percent_decode_query_value, required_session_query},
};
use crate::core::{
    JournalEntry, JournalRecord, ModelResponseOutcome, SessionStore, ToolCallRecordPhase,
    TurnSettlementStatus, canonicalize_session_dir_path,
};

pub(super) async fn read(state: &HttpAppState, query: Option<&str>) -> Result<AppSessionAnalysis> {
    let (session_dir, requested) = parse(query)?;
    let session_dir = state.resolve_session_dir(session_dir).await?;
    if let Some(server) = state.server_for_session_dir(&session_dir).await {
        let store = server
            .runtime
            .session_store()
            .cloned()
            .ok_or_else(|| anyhow!("session has no storage"))?;
        return tokio::task::spawn_blocking(move || {
            let records = store.load_projection()?.records;
            project(store.session_id(), records, requested)
        })
        .await?;
    }
    read_stored(state.storage_service()?, session_dir, requested).await
}

fn parse(query: Option<&str>) -> Result<(PathBuf, Option<TurnId>)> {
    let session_dir = canonicalize_session_dir_path(required_session_query(query)?)?;
    let mut requested = None;
    for pair in query.unwrap_or_default().split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == "turn_id" {
            if requested.is_some() {
                return Err(anyhow!("duplicate turn_id query parameter"));
            }
            requested = Some(percent_decode_query_value(value)?.parse::<TurnId>()?);
        }
    }
    Ok((session_dir, requested))
}

async fn read_stored(
    storage: crate::core::StorageService,
    session_dir: PathBuf,
    requested: Option<TurnId>,
) -> Result<AppSessionAnalysis> {
    tokio::task::spawn_blocking(move || {
        let store = SessionStore::open(storage, session_dir)?;
        let records = store.load_projection()?.records;
        project(store.session_id(), records, requested)
    })
    .await?
}

fn project(
    session_id: proteus_contracts::domain::SessionId,
    records: Vec<JournalRecord>,
    requested: Option<TurnId>,
) -> Result<AppSessionAnalysis> {
    let mut turns: Vec<AppAnalysisTurn> = Vec::new();
    let mut indices = HashMap::new();
    for record in &records {
        let Some(turn_id) = record.turn_id else {
            continue;
        };
        match &record.entry {
            JournalEntry::ContextRequestRecorded(_) | JournalEntry::ContextResponseRecorded(_) => {}
            JournalEntry::TurnOpened(opened) => {
                indices.insert(turn_id, turns.len());
                turns.push(AppAnalysisTurn {
                    turn_id,
                    number: turns.len() + 1,
                    prompt_preview: opened.task.text.chars().take(180).collect(),
                    started_at_ms: record.timestamp_ms,
                    finished_at_ms: None,
                    status: AppAnalysisTurnStatus::Unsettled,
                });
            }
            JournalEntry::TurnSettled(settled) => {
                if let Some(&index) = indices.get(&turn_id) {
                    turns[index].finished_at_ms = Some(record.timestamp_ms);
                    turns[index].status = match settled.status {
                        TurnSettlementStatus::Success => AppAnalysisTurnStatus::Success,
                        TurnSettlementStatus::Error => AppAnalysisTurnStatus::Error,
                        TurnSettlementStatus::Canceled => AppAnalysisTurnStatus::Canceled,
                        TurnSettlementStatus::Timeout => AppAnalysisTurnStatus::Timeout,
                    };
                }
            }
            _ => {}
        }
    }
    if requested.is_some_and(|id| !indices.contains_key(&id)) {
        return Err(anyhow!("requested turn does not belong to this session"));
    }
    let selected_id = requested.or_else(|| turns.last().map(|turn| turn.turn_id));
    let revision = records.last().map_or(0, |record| record.session_seq);
    let selected = selected_id
        .map(|id| turn_details(records, id))
        .transpose()?;
    Ok(AppSessionAnalysis {
        session_id,
        revision,
        turns,
        selected,
    })
}

fn turn_details(mut records: Vec<JournalRecord>, turn_id: TurnId) -> Result<AppTurnAnalysis> {
    let mut detail = None;
    let mut steps: Vec<AppAnalysisStep> = Vec::new();
    let mut models = HashMap::new();
    let mut tools = HashMap::new();
    // Drop unrelated payloads in place, without moving every large journal
    // entry through the selected turn's detail loop.
    records.retain(|record| record.turn_id == Some(turn_id));
    for record in records {
        let new_step = |data| AppAnalysisStep {
            sequence: record.session_seq,
            execution_id: record.execution_id,
            thread_id: record.thread_id,
            started_at_ms: record.timestamp_ms,
            finished_at_ms: None,
            data,
        };
        match record.entry {
            JournalEntry::ContextRequestRecorded(_) | JournalEntry::ContextResponseRecorded(_) => {}
            JournalEntry::TurnOpened(opened) => {
                detail = Some(AppTurnAnalysis {
                    turn_id,
                    task: opened.task,
                    module_epoch: opened.module_epoch,
                    config_snapshot: opened.config_snapshot,
                    steps: Vec::new(),
                    output: None,
                    error: None,
                });
            }
            JournalEntry::HookInvoked(mut trace) => {
                if matches!(
                    trace.input.event,
                    crate::contracts::HookEvent::BeforeContext { .. }
                ) && trace.steps.iter().any(|step| {
                    matches!(
                        step.outcome,
                        crate::contracts::HookStepOutcome::Accepted {
                            response:
                                crate::contracts::HookResponse::ContextPreparationDiagnostics { .. }
                        }
                    )
                }) {
                    let mut event = trace.input.event.clone();
                    for contribution in &mut trace.steps {
                        if let crate::contracts::HookStepOutcome::Accepted { response } =
                            &mut contribution.outcome
                        {
                            let expanded = crate::contracts::expand_context_preparation_response(
                                &event, response,
                            )?;
                            event = crate::contracts::apply_hook_response(&event, response)?;
                            *response = expanded;
                        }
                    }
                }
                let mut step = new_step(AppAnalysisStepData::Hook {
                    trace: Box::new(trace),
                });
                step.finished_at_ms = Some(record.timestamp_ms);
                steps.push(step);
            }
            JournalEntry::ModelRequestRecorded(model) => {
                models.insert(model.exchange_id, steps.len());
                steps.push(new_step(AppAnalysisStepData::Model {
                    exchange_id: model.exchange_id,
                    origin: model.origin,
                    request: Box::new(model.request),
                    response: None,
                    failure: None,
                    messages: Vec::new(),
                }));
            }
            JournalEntry::ModelMessageRecorded(message) => {
                if let Some(&index) = models.get(&message.exchange_id)
                    && let AppAnalysisStepData::Model { messages, .. } = &mut steps[index].data
                {
                    messages.push(message.message);
                }
            }
            JournalEntry::ModelResponseRecorded(model) => {
                if let Some(&index) = models.get(&model.exchange_id) {
                    let step = &mut steps[index];
                    step.finished_at_ms = Some(record.timestamp_ms);
                    if let AppAnalysisStepData::Model {
                        response, failure, ..
                    } = &mut step.data
                    {
                        match model.outcome {
                            ModelResponseOutcome::Response { response: value } => {
                                *response = Some(Box::new(value))
                            }
                            ModelResponseOutcome::Error { failure: value } => {
                                *failure = Some(value)
                            }
                        }
                    }
                }
            }
            JournalEntry::ToolCallRecorded(tool) => {
                let key = (record.execution_id, tool.call.id.clone());
                let index = *tools.entry(key).or_insert_with(|| {
                    steps.push(new_step(AppAnalysisStepData::Tool {
                        call: tool.call,
                        approval_reason: None,
                        resolution: None,
                        result: None,
                    }));
                    steps.len() - 1
                });
                if let AppAnalysisStepData::Tool {
                    approval_reason,
                    resolution,
                    ..
                } = &mut steps[index].data
                {
                    match tool.phase {
                        ToolCallRecordPhase::Requested { .. } => {}
                        ToolCallRecordPhase::ApprovalRequested { reason } => {
                            *approval_reason = Some(reason)
                        }
                        ToolCallRecordPhase::Resolved { resolution: value } => {
                            *resolution = Some(value)
                        }
                    }
                }
            }
            JournalEntry::ToolEffectRecorded(tool) | JournalEntry::ToolResultRecorded(tool) => {
                if let Some(&index) = tools.get(&(record.execution_id, tool.result.call_id.clone()))
                {
                    steps[index].finished_at_ms = Some(record.timestamp_ms);
                    if let AppAnalysisStepData::Tool { result, .. } = &mut steps[index].data {
                        *result = Some(tool.result);
                    }
                }
            }
            JournalEntry::HistoryMutated(history) => {
                if let Some(report) = history.compaction {
                    let mut step = new_step(AppAnalysisStepData::Compaction {
                        report,
                        history_revision: history.new_revision,
                    });
                    step.finished_at_ms = Some(record.timestamp_ms);
                    steps.push(step);
                }
            }
            JournalEntry::TurnSettled(settled) => {
                if let Some(detail) = &mut detail {
                    detail.output = settled.output;
                    detail.error = settled.error;
                }
            }
            JournalEntry::SubagentSessionBoundRecorded(_) => {}
        }
    }
    let mut detail = detail.ok_or_else(|| anyhow!("turn has no recorded admission"))?;
    detail.steps = steps;
    Ok(detail)
}

#[cfg(test)]
mod tests;
