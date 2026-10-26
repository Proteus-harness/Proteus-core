mod build;
mod part_identity;
#[cfg(test)]
mod retention_tests;
mod sequence;

use im::{HashMap as SharedMap, HashSet as SharedSet};
use std::{collections::HashSet, sync::Arc};

use anyhow::{Result, bail};
use proteus_contracts::{
    domain::{CallId, ExchangeId, ExecutionId, PartId, RecordId, SessionId, ThreadId, TurnId},
    model_standard::{CanonicalMessage, PartScope},
};

use super::types::{
    HistoryMutationKind, JOURNAL_SCHEMA_VERSION, JournalEntry, JournalRecord, ToolCallRecordPhase,
};

#[derive(Debug, Clone, Default)]
// Append clones this candidate before validation. Shared indices and immutable
// payloads keep that transaction cheap; mutations must preserve COW isolation.
pub(crate) struct JournalValidationState {
    capture: Arc<super::history_capture::HistoryCapture>,
    capture_owner: Option<JournalRecordAttributionOwner>,
    history: Arc<Vec<CanonicalMessage>>,
    history_revision: u64,
    history_turn: Option<TurnId>,
    interrupted_turns: Arc<Vec<proteus_contracts::contracts::WorkflowHistoryInterruption>>,
    record_ids: SharedSet<RecordId>,
    known_parts: SharedMap<PartId, part_identity::PartIdentity>,
    opened_turns: SharedMap<TurnId, TurnLifecycle>,
    execution_turns: SharedMap<ExecutionId, TurnId>,
    detached_executions: SharedSet<ExecutionId>,
    settled_turns: SharedSet<TurnId>,
    model_requests: SharedMap<ExchangeId, ExecutionFactOwner>,
    model_responses: SharedSet<ExchangeId>,
    context: super::context::ContextLifecycle,
    tool_calls: SharedMap<CallId, Arc<ToolLifecycle>>,
}

type JournalRecordAttributionOwner = (ThreadId, TurnId);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TurnLifecycle {
    root_thread_id: ThreadId,
    execution_id: ExecutionId,
    history_compactions: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ExecutionFactOwner {
    execution_id: ExecutionId,
    thread_id: Option<ThreadId>,
    turn_id: Option<TurnId>,
}

#[derive(Debug, Clone, Default)]
struct ToolLifecycle {
    owner: Option<ExecutionFactOwner>,
    call: Option<proteus_contracts::domain::ToolCall>,
    hook_reviewed: bool,
    approval_requested: bool,
    resolved: bool,
    result: bool,
    effect: Option<proteus_contracts::domain::ToolResult>,
}

impl JournalValidationState {
    pub(crate) fn apply(&mut self, record: &JournalRecord) -> Result<()> {
        if self.record_ids.insert(record.record_id).is_some() {
            bail!("duplicate journal record_id {}", record.record_id);
        }

        match &record.entry {
            JournalEntry::HookInvoked(trace) => {
                let owner = self.require_execution_fact(record)?;
                super::hooks::validate_trace(record, trace)?;
                if let proteus_contracts::contracts::HookEvent::BeforeTool { call, .. } =
                    &trace.input.event
                {
                    let lifecycle = self.tool_calls.get_mut(&call.id).ok_or_else(|| {
                        anyhow::anyhow!("tool hook precedes requested call {}", call.id)
                    })?;
                    let lifecycle = Arc::make_mut(lifecycle);
                    if lifecycle.owner != Some(owner)
                        || lifecycle.call.as_ref() != Some(call)
                        || lifecycle.hook_reviewed
                        || lifecycle.approval_requested
                        || lifecycle.resolved
                    {
                        bail!("tool hook conflicts with requested call {}", call.id);
                    }
                    lifecycle.hook_reviewed = true;
                    if let Some(proteus_contracts::contracts::HookEvent::BeforeTool {
                        call, ..
                    }) = &trace.output
                    {
                        lifecycle.call = Some(call.clone());
                    }
                }
                for messages in super::hooks::model_messages(trace) {
                    Self::validate_part_id_stability(&mut self.known_parts, messages)?;
                }
            }
            JournalEntry::TurnOpened(opened) => {
                self.capture = Default::default();
                self.capture_owner = None;
                let turn_id = required_turn_id(record)?;
                let thread_id = required_thread_id(record)?;
                let execution_id = required_execution_id(record)?;
                if self
                    .opened_turns
                    .insert(
                        turn_id,
                        TurnLifecycle {
                            root_thread_id: thread_id,
                            execution_id,
                            history_compactions: 0,
                        },
                    )
                    .is_some()
                {
                    bail!("turn {turn_id} was opened more than once");
                }
                if self.detached_executions.contains(&execution_id) {
                    bail!(
                        "detached execution {execution_id} cannot later be bound to turn {turn_id}"
                    );
                }
                if let Some(previous_turn) = self.execution_turns.insert(execution_id, turn_id) {
                    bail!(
                        "execution {execution_id} was already bound to turn {previous_turn}, cannot open turn {turn_id}"
                    );
                }
                if opened.base_history_revision != self.history_revision {
                    bail!(
                        "turn {turn_id} opened at history revision {}, current revision is {}",
                        opened.base_history_revision,
                        self.history_revision
                    );
                }
            }
            JournalEntry::HistoryMutated(mutation) => {
                reject_execution_id(record)?;
                if record.turn_id.is_some() {
                    self.require_root_turn(record)?;
                }
                if mutation.previous_revision != self.history_revision {
                    bail!(
                        "history revision mismatch: record expects {}, current revision is {}",
                        mutation.previous_revision,
                        self.history_revision
                    );
                }
                if mutation.new_revision != mutation.previous_revision.saturating_add(1) {
                    bail!(
                        "history revision must increment by one: {} -> {}",
                        mutation.previous_revision,
                        mutation.new_revision
                    );
                }
                validate_conversation_messages(&mutation.messages)?;
                Self::validate_part_id_stability(&mut self.known_parts, &mutation.messages)?;
                if mutation.mutation != HistoryMutationKind::Checkpoint
                    && !mutation.tool_results.is_empty()
                {
                    bail!("only a workflow checkpoint may bind tool results");
                }
                match mutation.mutation {
                    HistoryMutationKind::Append => {
                        if mutation.messages.is_empty() {
                            bail!("history append must contain at least one message");
                        }
                        if mutation.compaction.is_some() {
                            bail!("history append cannot carry a compaction report");
                        }
                        Arc::make_mut(&mut self.history).extend(mutation.messages.iter().cloned());
                    }
                    HistoryMutationKind::Replace => {
                        self.capture = Default::default();
                        self.capture_owner = None;
                        if mutation
                            .compaction
                            .as_ref()
                            .is_some_and(|report| !report.changed)
                        {
                            bail!("history replacement compaction report must be changed=true");
                        }
                        self.history = Arc::new(mutation.messages.clone());
                    }
                    HistoryMutationKind::Checkpoint => {
                        let turn_id = self.require_root_turn(record)?;
                        if mutation.tool_results.iter().any(|binding| {
                            self.known_parts.contains_key(&binding.part_id)
                                && !self.capture.has_binding(binding)
                        }) {
                            bail!("checkpoint reuses a previously recorded result part identity");
                        }
                        let (capture, history) = self.capture.rebase(
                            &self.history,
                            &mutation.messages,
                            &mutation.tool_results,
                            mutation
                                .compaction
                                .as_ref()
                                .is_some_and(|report| report.changed),
                        )?;
                        self.capture = Arc::new(capture);
                        self.history = Arc::new(history);
                        self.capture_owner = Some((required_thread_id(record)?, turn_id));
                    }
                }
                validate_active_history_ids(&self.history)?;
                if mutation
                    .compaction
                    .as_ref()
                    .is_some_and(|report| report.changed)
                {
                    if let Some(turn_id) = record.turn_id {
                        // require_root_turn above already validated this owner.
                        self.opened_turns
                            .get_mut(&turn_id)
                            .expect("validated root turn")
                            .history_compactions += 1;
                    }
                    self.interrupted_turns = Arc::default();
                }
                self.history_revision = mutation.new_revision;
                self.history_turn = record.turn_id;
            }
            JournalEntry::ContextRequestRecorded(request) => {
                let owner = self.require_execution_fact(record)?;
                self.context.request(request.invocation_id, owner)?;
            }
            JournalEntry::ContextResponseRecorded(response) => {
                let owner = self.require_execution_fact(record)?;
                self.context.response(response.invocation_id, owner)?;
            }
            JournalEntry::ModelRequestRecorded(request) => {
                let owner = self.require_execution_fact(record)?;
                Self::validate_part_id_stability(&mut self.known_parts, &request.request.messages)?;
                if self
                    .model_requests
                    .insert(request.exchange_id, owner)
                    .is_some()
                {
                    bail!("duplicate model exchange request {}", request.exchange_id);
                }
            }
            JournalEntry::ModelMessageRecorded(item) => {
                let owner = self.require_execution_fact(record)?;
                if self.model_requests.get(&item.exchange_id) != Some(&owner)
                    || self.model_responses.contains(&item.exchange_id)
                {
                    bail!("completed model message has no active exchange with the same owner");
                }
                Self::validate_part_id_stability(
                    &mut self.known_parts,
                    std::slice::from_ref(&item.message),
                )?;
            }
            JournalEntry::ModelResponseRecorded(response) => {
                let owner = self.require_execution_fact(record)?;
                let Some(request_owner) = self.model_requests.get(&response.exchange_id) else {
                    bail!(
                        "model response {} has no preceding request",
                        response.exchange_id
                    );
                };
                if request_owner != &owner {
                    bail!(
                        "model response {} changed lifecycle owner",
                        response.exchange_id
                    );
                }
                if self.model_responses.insert(response.exchange_id).is_some() {
                    bail!("duplicate model exchange response {}", response.exchange_id);
                }
                if let super::types::ModelResponseOutcome::Response { response } = &response.outcome
                {
                    Self::validate_part_id_stability(&mut self.known_parts, &response.messages)?;
                }
            }
            JournalEntry::ToolCallRecorded(tool) => {
                let owner = self.require_execution_fact(record)?;
                if matches!(
                    &tool.phase,
                    ToolCallRecordPhase::Requested {
                        owner: crate::contracts::OperationOwner::Workflow
                    }
                ) && record.thread_id.zip(record.turn_id) == self.capture_owner
                {
                    self.capture.validate_call(&tool.call)?;
                }
                let lifecycle =
                    Arc::make_mut(self.tool_calls.entry(tool.call.id.clone()).or_default());
                match &tool.phase {
                    ToolCallRecordPhase::Requested { .. } => {
                        if lifecycle.call.is_some() {
                            bail!("tool call {} was requested more than once", tool.call.id);
                        }
                        lifecycle.owner = Some(owner);
                        lifecycle.call = Some(tool.call.clone());
                    }
                    ToolCallRecordPhase::ApprovalRequested { .. } => {
                        let Some(requested) = lifecycle.call.as_ref() else {
                            bail!(
                                "tool call {} requested approval before it was recorded",
                                tool.call.id
                            );
                        };
                        if requested != &tool.call {
                            bail!("tool call {} changed before approval request", tool.call.id);
                        }
                        if lifecycle.owner != Some(owner) {
                            bail!("tool call {} changed lifecycle owner", tool.call.id);
                        }
                        if lifecycle.approval_requested {
                            bail!(
                                "tool call {} requested approval more than once",
                                tool.call.id
                            );
                        }
                        if lifecycle.resolved {
                            bail!(
                                "tool call {} requested approval after resolution",
                                tool.call.id
                            );
                        }
                        lifecycle.approval_requested = true;
                    }
                    ToolCallRecordPhase::Resolved { resolution } => {
                        let Some(requested) = lifecycle.call.as_ref() else {
                            bail!(
                                "tool call {} resolved before it was requested",
                                tool.call.id
                            );
                        };
                        if requested != &tool.call {
                            bail!(
                                "tool call {} changed between request and resolution",
                                tool.call.id
                            );
                        }
                        if lifecycle.owner != Some(owner) {
                            bail!("tool call {} changed lifecycle owner", tool.call.id);
                        }
                        if lifecycle.resolved {
                            bail!("tool call {} was resolved more than once", tool.call.id);
                        }
                        if resolution.requested_approval() != lifecycle.approval_requested {
                            bail!(
                                "tool call {} approval lifecycle does not match resolution {:?}",
                                tool.call.id,
                                resolution
                            );
                        }
                        lifecycle.resolved = true;
                    }
                }
            }
            JournalEntry::ToolEffectRecorded(tool) => {
                let owner = self.require_execution_fact(record)?;
                let Some(lifecycle) = self.tool_calls.get_mut(&tool.result.call_id) else {
                    bail!("tool effect {} has no preceding call", tool.result.call_id);
                };
                let lifecycle = Arc::make_mut(lifecycle);
                if !lifecycle.resolved
                    || lifecycle.owner != Some(owner)
                    || lifecycle.effect.is_some()
                    || lifecycle.result
                {
                    bail!(
                        "tool effect {} violates its call lifecycle",
                        tool.result.call_id
                    );
                }
                lifecycle.effect = Some(tool.result.clone());
            }
            JournalEntry::ToolResultRecorded(tool) => {
                let owner = self.require_execution_fact(record)?;
                let Some(lifecycle) = self.tool_calls.get_mut(&tool.result.call_id) else {
                    bail!("tool result {} has no preceding call", tool.result.call_id);
                };
                let lifecycle = Arc::make_mut(lifecycle);
                if !lifecycle.resolved {
                    bail!(
                        "tool result {} precedes call resolution",
                        tool.result.call_id
                    );
                }
                if lifecycle.owner != Some(owner) {
                    bail!(
                        "tool result {} changed lifecycle owner",
                        tool.result.call_id
                    );
                }
                if lifecycle.result {
                    bail!("duplicate tool result {}", tool.result.call_id);
                }
                if let Some(actual) = &lifecycle.effect
                    && (actual.ok != tool.result.ok
                        || actual.error != tool.result.error
                        || actual.content != tool.result.content)
                {
                    bail!("tool result changed its actual status/error/content");
                }
                lifecycle.result = true;
                // The effect is only needed until the final result is checked.
                lifecycle.effect = None;
                if record.thread_id.zip(record.turn_id) == self.capture_owner
                    && self.capture_tool_result(&tool.result)?
                {
                    self.history_revision = self.history_revision.saturating_add(1);
                    validate_active_history_ids(&self.history)?;
                    Self::validate_part_id_stability(&mut self.known_parts, &self.history)?;
                }
            }
            JournalEntry::TurnSettled(settled) => {
                reject_execution_id(record)?;
                let turn_id = self.require_root_turn(record)?;
                if self.capture_owner == record.thread_id.zip(record.turn_id) {
                    let pending = self
                        .tool_calls
                        .values()
                        .filter(|call| {
                            !call.result
                                && call.owner.is_some_and(|owner| {
                                    owner.thread_id == record.thread_id
                                        && owner.turn_id == record.turn_id
                                })
                        })
                        .filter_map(|call| call.effect.clone())
                        .collect::<Vec<_>>();
                    for result in pending {
                        if self.capture_tool_result(&result)? {
                            self.history_revision = self.history_revision.saturating_add(1);
                            validate_active_history_ids(&self.history)?;
                            Self::validate_part_id_stability(&mut self.known_parts, &self.history)?;
                        }
                    }
                }
                if self.settled_turns.insert(turn_id).is_some() {
                    bail!("turn {turn_id} settled more than once");
                }
                if settled.status == super::types::TurnSettlementStatus::Canceled
                    && self.history_turn == Some(turn_id)
                    && let Some(message) = self.history.last()
                {
                    Arc::make_mut(&mut self.interrupted_turns).push(
                        proteus_contracts::contracts::WorkflowHistoryInterruption {
                            turn_id,
                            after_message_id: message.id,
                        },
                    );
                }
                if self.capture_owner == record.thread_id.zip(record.turn_id) {
                    self.capture = Default::default();
                    self.capture_owner = None;
                }
            }
        }
        Ok(())
    }

    fn capture_tool_result(
        &mut self,
        result: &proteus_contracts::domain::ToolResult,
    ) -> Result<bool> {
        if !self.capture.contains_call(&result.call_id) {
            return Ok(false);
        }
        Arc::make_mut(&mut self.capture).record(Arc::make_mut(&mut self.history), result)
    }

    pub(crate) fn interrupted_turns(
        &self,
    ) -> Vec<proteus_contracts::contracts::WorkflowHistoryInterruption> {
        self.interrupted_turns.as_ref().clone()
    }

    pub(crate) fn history_revision(&self) -> u64 {
        self.history_revision
    }

    pub(crate) fn history(&self) -> &[CanonicalMessage] {
        &self.history
    }

    pub(crate) fn committed_history(
        &self,
        thread_id: ThreadId,
        turn_id: TurnId,
    ) -> (Vec<CanonicalMessage>, usize) {
        (
            self.history.as_ref().clone(),
            self.opened_turns
                .get(&turn_id)
                .filter(|turn| turn.root_thread_id == thread_id)
                .map(|turn| turn.history_compactions)
                .unwrap_or(0),
        )
    }

    fn validate_part_id_stability(
        known_parts: &mut SharedMap<PartId, part_identity::PartIdentity>,
        messages: &[CanonicalMessage],
    ) -> Result<()> {
        for message in messages {
            for part in &message.parts {
                if let Some(previous) = known_parts.get(&part.part_id) {
                    if !previous.matches(part) {
                        bail!(
                            "canonical part {} changed after it was recorded",
                            part.part_id
                        );
                    }
                } else {
                    known_parts.insert(part.part_id, part_identity::PartIdentity::new(part));
                }
            }
        }
        Ok(())
    }

    fn require_execution_fact(&mut self, record: &JournalRecord) -> Result<ExecutionFactOwner> {
        let execution_id = required_execution_id(record)?;
        match (record.thread_id, record.turn_id) {
            (None, None) => {
                if let Some(turn_id) = self.execution_turns.get(&execution_id) {
                    bail!(
                        "execution {execution_id} is bound to turn {turn_id} and cannot emit detached facts"
                    );
                }
                self.detached_executions.insert(execution_id);
                Ok(ExecutionFactOwner {
                    execution_id,
                    thread_id: None,
                    turn_id: None,
                })
            }
            (Some(thread_id), Some(turn_id)) => {
                let Some(turn) = self.opened_turns.get(&turn_id) else {
                    bail!(
                        "journal record {} ({:?}) attributes execution {execution_id} to turn {turn_id} before it was opened",
                        record.record_id,
                        record.entry.kind()
                    );
                };
                if turn.execution_id != execution_id {
                    bail!(
                        "journal record {} ({:?}) maps turn {turn_id} to execution {execution_id}, expected {}",
                        record.record_id,
                        record.entry.kind(),
                        turn.execution_id
                    );
                }
                if self.settled_turns.contains(&turn_id) && turn.root_thread_id == thread_id {
                    bail!(
                        "journal record {} ({:?}) references settled root turn {turn_id}",
                        record.record_id,
                        record.entry.kind()
                    );
                }
                Ok(ExecutionFactOwner {
                    execution_id,
                    thread_id: Some(thread_id),
                    turn_id: Some(turn_id),
                })
            }
            _ => bail!(
                "journal record {} ({:?}) must provide both thread_id and turn_id for agent attribution, or neither for detached execution",
                record.record_id,
                record.entry.kind()
            ),
        }
    }

    fn require_root_turn(&self, record: &JournalRecord) -> Result<TurnId> {
        let turn_id = required_turn_id(record)?;
        let Some(turn) = self.opened_turns.get(&turn_id) else {
            bail!("turn {turn_id} settled or mutated history before it was opened");
        };
        if Some(turn.root_thread_id) != record.thread_id {
            bail!("turn {turn_id} root lifecycle changed thread_id");
        }
        if self.settled_turns.contains(&turn_id) {
            bail!("turn {turn_id} was already settled");
        }
        Ok(turn_id)
    }
}

#[derive(Debug, Clone)]
pub struct JournalProjection {
    pub records: Vec<JournalRecord>,
    pub history: Vec<CanonicalMessage>,
    pub history_revision: u64,
    pub interrupted_turns: Vec<proteus_contracts::contracts::WorkflowHistoryInterruption>,
    pub interrupted_model_exchanges: Vec<ExchangeId>,
    pub interrupted_context_builds: Vec<uuid::Uuid>,
    pub unresolved_tool_calls: Vec<CallId>,
    pub unsettled_turns: Vec<TurnId>,
}

fn required_turn_id(record: &JournalRecord) -> Result<TurnId> {
    record.turn_id.ok_or_else(|| {
        anyhow::anyhow!(
            "journal record {} ({:?}) requires turn_id",
            record.record_id,
            record.entry.kind()
        )
    })
}

fn required_thread_id(record: &JournalRecord) -> Result<ThreadId> {
    record.thread_id.ok_or_else(|| {
        anyhow::anyhow!(
            "journal record {} ({:?}) requires thread_id",
            record.record_id,
            record.entry.kind()
        )
    })
}

fn required_execution_id(record: &JournalRecord) -> Result<ExecutionId> {
    record.execution_id.ok_or_else(|| {
        anyhow::anyhow!(
            "journal record {} ({:?}) requires execution_id",
            record.record_id,
            record.entry.kind()
        )
    })
}

fn reject_execution_id(record: &JournalRecord) -> Result<()> {
    if record.execution_id.is_some() {
        bail!(
            "journal record {} ({:?}) is a chat/session lifecycle fact and must not carry execution_id",
            record.record_id,
            record.entry.kind()
        );
    }
    Ok(())
}

fn validate_conversation_messages(messages: &[CanonicalMessage]) -> Result<()> {
    for message in messages {
        for part in &message.parts {
            if part.scope != PartScope::Conversation {
                bail!(
                    "history message {} contains non-conversation part {} with scope {:?}",
                    message.id,
                    part.part_id,
                    part.scope
                );
            }
        }
    }
    Ok(())
}

fn validate_active_history_ids(messages: &[CanonicalMessage]) -> Result<()> {
    let mut message_ids = HashSet::new();
    let mut part_ids = HashSet::new();
    for message in messages {
        if !message_ids.insert(message.id) {
            bail!(
                "active history contains duplicate message id {}",
                message.id
            );
        }
        for part in &message.parts {
            if !part_ids.insert(part.part_id) {
                bail!(
                    "active history contains duplicate part id {} at message {}",
                    part.part_id,
                    message.id,
                );
            }
        }
    }
    Ok(())
}
