mod build;
#[cfg(test)]
mod capture_tests;
mod facts;
mod part_identity;
#[cfg(test)]
mod retention_tests;
mod sequence;
mod subagent;

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
pub struct JournalValidationState {
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
    permitted: bool,
    // Allocate a reference only for subagent calls; ordinary tool lifecycles
    // must not reserve inline PathBuf/String/UUID payloads.
    subagent_binding: Option<Arc<proteus_contracts::domain::SubagentSessionBinding>>,
    binding_recorded: bool,
    result: bool,
    effect: Option<proteus_contracts::domain::ToolResult>,
}

impl JournalValidationState {
    /// Stable record identity permits a storage worker to reconcile retries
    /// without keeping a second index or scanning on every fresh append.
    pub fn contains_record_id(&self, record_id: &RecordId) -> bool {
        self.record_ids.contains(record_id)
    }

    pub fn interrupted_turns(
        &self,
    ) -> Vec<proteus_contracts::contracts::WorkflowHistoryInterruption> {
        self.interrupted_turns.as_ref().clone()
    }

    pub fn history_revision(&self) -> u64 {
        self.history_revision
    }

    pub fn history(&self) -> &[CanonicalMessage] {
        &self.history
    }

    pub fn last_history_turn(&self) -> Option<TurnId> {
        self.history_turn
    }

    pub fn into_history(self) -> Vec<CanonicalMessage> {
        Arc::unwrap_or_clone(self.history)
    }

    pub fn committed_history(
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
