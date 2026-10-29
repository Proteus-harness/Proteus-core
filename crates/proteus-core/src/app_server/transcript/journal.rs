use std::collections::{HashMap, HashSet};

mod timestamps;
use timestamps::TranscriptTimestamps;

use crate::{
    core::{
        HistoryMutationKind, JournalEntry, JournalProjection, ModelResponseOutcome,
        ToolCallRecordPhase, TurnSettled, TurnSettlementStatus,
    },
    domain::{CallId, PartId, ThreadId, ToolCallResolution, TurnId},
    model_standard::{CanonicalMessage, ContentPart, PartProvenance, PartScope},
};

use super::{
    AppTranscriptMessage, append_transcript_message, append_transcript_tool_call,
    append_transcript_tool_result, finalize_interrupted_tools,
};

/// Строит завершённый web transcript из canonical journal. Active streaming
/// turn проецируется отдельно через in-memory `turn_progress`: здесь records
/// уже являются единственным durable источником фактов.
pub(crate) fn journal_transcript_messages(
    projection: &JournalProjection,
    live_turn_id: Option<TurnId>,
) -> Vec<AppTranscriptMessage> {
    let visibility = TranscriptVisibility::from_projection(projection);
    let mut state = TranscriptProjectionState {
        timestamps: TranscriptTimestamps::from_projection(projection),
        ..Default::default()
    };
    let final_results = projection
        .records
        .iter()
        .filter_map(|record| match &record.entry {
            JournalEntry::ToolResultRecorded(tool) => Some(tool.result.call_id.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();

    for record in &projection.records {
        // The live turn is owned by the inline event projection until settlement.
        if live_turn_id.is_some() && record.turn_id == live_turn_id {
            continue;
        }
        match &record.entry {
            JournalEntry::HistoryMutated(mutation) => {
                if mutation.mutation == HistoryMutationKind::Replace
                    && mutation.compaction.is_none()
                {
                    state.reset();
                }
                let hide_compactor_parts = mutation.mutation != HistoryMutationKind::Append
                    && mutation.compaction.is_some();
                for message in &mutation.messages {
                    state.append_message(message, hide_compactor_parts);
                }
            }
            JournalEntry::ModelResponseRecorded(response)
                if record.turn_id != live_turn_id
                    && visibility.is_root_record(record.thread_id, record.turn_id)
                    && visibility
                        .visible_model_responses
                        .contains(&response.exchange_id) =>
            {
                if let ModelResponseOutcome::Response { response } = &response.outcome {
                    for message in &response.messages {
                        state.append_message(message, false);
                    }
                }
            }
            JournalEntry::ToolCallRecorded(tool)
                if record.turn_id != live_turn_id
                    && visibility.is_root_record(record.thread_id, record.turn_id)
                    && visibility.visible_tool_calls.contains(&tool.call.id) =>
            {
                match &tool.phase {
                    ToolCallRecordPhase::Requested { .. } => state.append_tool_call(&tool.call),
                    ToolCallRecordPhase::Resolved { resolution } => {
                        state.apply_resolution(&tool.call, resolution)
                    }
                    ToolCallRecordPhase::ApprovalRequested { .. } => {}
                }
            }
            JournalEntry::ToolResultRecorded(tool)
                if record.turn_id != live_turn_id
                    && visibility.is_root_record(record.thread_id, record.turn_id)
                    && visibility.visible_tool_calls.contains(&tool.result.call_id) =>
            {
                if state.seen_results.insert(tool.result.call_id.clone()) {
                    state.append_tool_result(&tool.result);
                }
            }
            JournalEntry::ToolEffectRecorded(tool)
                if record.turn_id != live_turn_id
                    && visibility.is_root_record(record.thread_id, record.turn_id)
                    && !final_results.contains(&tool.result.call_id)
                    && visibility.visible_tool_calls.contains(&tool.result.call_id) =>
            {
                if state.seen_results.insert(tool.result.call_id.clone()) {
                    state.append_tool_result(&tool.result);
                }
            }
            JournalEntry::TurnSettled(settled)
                if record.turn_id != live_turn_id
                    && visibility.is_root_record(record.thread_id, record.turn_id) =>
            {
                state.append_turn_failure(settled, record.timestamp_ms);
            }
            _ => {}
        }
    }

    finalize_interrupted_tools(&mut state.transcript);
    state.transcript
}

#[derive(Default)]
struct TranscriptVisibility {
    root_threads: HashMap<TurnId, ThreadId>,
    visible_tool_calls: HashSet<CallId>,
    visible_model_responses: HashSet<crate::domain::ExchangeId>,
}

impl TranscriptVisibility {
    fn from_projection(projection: &JournalProjection) -> Self {
        let mut root_threads = HashMap::new();
        let mut committed_parts = HashSet::new();
        let mut root_lifecycle_calls = HashSet::new();

        for record in &projection.records {
            if matches!(record.entry, JournalEntry::TurnOpened(_))
                && let Some(turn_id) = record.turn_id
                && let Some(thread_id) = record.thread_id
            {
                root_threads.insert(turn_id, thread_id);
            }
            if let JournalEntry::HistoryMutated(mutation) = &record.entry {
                for message in &mutation.messages {
                    committed_parts.extend(message.parts.iter().map(|part| part.part_id));
                }
            }
        }

        for record in &projection.records {
            if let JournalEntry::ToolCallRecorded(tool) = &record.entry
                && matches!(
                    &tool.phase,
                    ToolCallRecordPhase::Requested {
                        owner: crate::contracts::OperationOwner::Workflow
                    }
                )
                && is_root_record(&root_threads, record.thread_id, record.turn_id)
            {
                root_lifecycle_calls.insert(tool.call.id.clone());
            }
        }

        let mut visible_model_responses = HashSet::new();
        for record in &projection.records {
            let JournalEntry::ModelResponseRecorded(response) = &record.entry else {
                continue;
            };
            if !is_root_record(&root_threads, record.thread_id, record.turn_id) {
                continue;
            }
            let ModelResponseOutcome::Response { response: model } = &response.outcome else {
                continue;
            };
            let committed = model
                .messages
                .iter()
                .flat_map(|message| message.parts.iter())
                .any(|part| committed_parts.contains(&part.part_id));
            let executed = model
                .tool_calls
                .iter()
                .any(|call| root_lifecycle_calls.contains(&call.id));
            if committed || executed {
                visible_model_responses.insert(response.exchange_id);
            }
        }

        Self {
            root_threads,
            visible_tool_calls: root_lifecycle_calls,
            visible_model_responses,
        }
    }

    fn is_root_record(&self, thread_id: Option<ThreadId>, turn_id: Option<TurnId>) -> bool {
        is_root_record(&self.root_threads, thread_id, turn_id)
    }
}

fn is_root_record(
    root_threads: &HashMap<TurnId, ThreadId>,
    thread_id: Option<ThreadId>,
    turn_id: Option<TurnId>,
) -> bool {
    turn_id
        .and_then(|turn_id| root_threads.get(&turn_id))
        .zip(thread_id)
        .is_some_and(|(root_thread_id, thread_id)| *root_thread_id == thread_id)
}

#[derive(Default)]
struct TranscriptProjectionState {
    timestamps: TranscriptTimestamps,
    transcript: Vec<AppTranscriptMessage>,
    seen_parts: HashSet<PartId>,
    seen_calls: HashSet<CallId>,
    seen_results: HashSet<CallId>,
}

impl TranscriptProjectionState {
    fn reset(&mut self) {
        self.transcript.clear();
        self.seen_parts.clear();
        self.seen_calls.clear();
        self.seen_results.clear();
    }

    fn append_message(&mut self, message: &CanonicalMessage, hide_compactor_parts: bool) {
        let timestamp_ms = self.timestamps.message(message);
        let mut message = message.clone();
        message.parts.retain(|part| {
            if !self.seen_parts.insert(part.part_id)
                || part.scope != PartScope::Conversation
                || (hide_compactor_parts && part.provenance == PartProvenance::Compactor)
            {
                return false;
            }
            match &part.payload {
                ContentPart::ToolCall { call } => self.seen_calls.insert(call.id.clone()),
                ContentPart::ToolResult { result } => {
                    self.seen_results.insert(result.call_id.clone())
                }
                _ => true,
            }
        });
        if !message.parts.is_empty() {
            let start = self.transcript.len();
            append_transcript_message(&mut self.transcript, &message);
            for row in &mut self.transcript[start..] {
                row.timestamp_ms = row
                    .tool
                    .as_ref()
                    .and_then(|tool| self.timestamps.call(&tool.call_id))
                    .or(timestamp_ms);
            }
        }
    }

    fn append_tool_call(&mut self, call: &crate::domain::ToolCall) {
        if self.seen_calls.insert(call.id.clone()) {
            append_transcript_tool_call(&mut self.transcript, call);
            self.transcript
                .last_mut()
                .expect("appended call")
                .timestamp_ms = self.timestamps.call(&call.id);
        }
    }

    fn append_tool_result(&mut self, result: &crate::domain::ToolResult) {
        let start = self.transcript.len();
        append_transcript_tool_result(&mut self.transcript, result);
        if self.transcript.len() > start {
            self.transcript[start].timestamp_ms = self.timestamps.call(&result.call_id);
        }
    }

    fn apply_resolution(
        &mut self,
        call: &crate::domain::ToolCall,
        resolution: &ToolCallResolution,
    ) {
        let Some(tool) = self
            .transcript
            .iter_mut()
            .rev()
            .filter_map(|message| message.tool.as_mut())
            .find(|tool| tool.call_id == call.id)
        else {
            return;
        };
        if resolution.permits_side_effect() {
            tool.effective_args = Some(call.args.clone());
            return;
        }
        let reason = match resolution {
            ToolCallResolution::ApprovalDenied { reason }
            | ToolCallResolution::PolicyDenied { reason }
            | ToolCallResolution::ValidationFailed { reason }
            | ToolCallResolution::Unsupported { reason } => reason.clone(),
            _ => "tool call was not permitted".to_owned(),
        };
        tool.status = "failed".to_owned();
        tool.result = Some(reason);
    }

    fn append_turn_failure(&mut self, settled: &TurnSettled, timestamp_ms: i64) {
        if settled.status == TurnSettlementStatus::Success {
            return;
        }
        // The status word lets clients tell a stop by the user from a failure.
        let (status, fallback) = match settled.status {
            TurnSettlementStatus::Success => return,
            TurnSettlementStatus::Error => ("error", "turn failed"),
            TurnSettlementStatus::Canceled => ("canceled", "turn canceled by client"),
            TurnSettlementStatus::Timeout => ("timeout", "turn timed out"),
        };
        let error = settled
            .error
            .as_deref()
            .map(str::trim)
            .filter(|error| !error.is_empty())
            .unwrap_or(fallback);
        self.transcript.push(AppTranscriptMessage {
            timestamp_ms: Some(timestamp_ms),
            images: Vec::new(),
            message_id: None,
            phase: None,
            role: "system".to_owned(),
            text: format!("AppServer {status}: {error}"),
            tool: None,
            subagent: None,
            streaming: false,
        });
    }
}

#[cfg(test)]
mod tests;
