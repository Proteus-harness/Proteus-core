//! Preparation candidates are separate from acknowledged working history.
use super::*;
use proteus_contracts::contracts::WorkflowCheckpointAck;

pub(super) struct HistoryState {
    pub(super) history: Option<WorkflowHistoryUpdate>,
    model_messages: Vec<CanonicalMessage>,
    persistent_messages: Vec<CanonicalMessage>,
    current_user_message_id: MessageId,
    current_turn_messages_start: usize,
    compactions: Vec<HistoryCompactionReport>,
    history_replacement_len: Option<usize>,
}

pub(super) struct PendingPreparation {
    pub(super) previous: HistoryState,
    pub(super) target: Option<ContextPreparationTarget>,
}

impl TurnScaffold {
    pub(super) fn stage_preparation(
        &mut self,
        target: Option<ContextPreparationTarget>,
    ) -> Result<(), ProcessModuleError> {
        if self.pending_preparation.is_some() {
            return Err(ProcessModuleError::new(
                "previous context preparation is still pending",
            ));
        }
        self.pending_preparation = Some(PendingPreparation {
            target,
            previous: HistoryState {
                history: self.history_update()?,
                model_messages: self.model_messages.clone(),
                persistent_messages: self.persistent_messages.clone(),
                current_user_message_id: self.current_user_message_id,
                current_turn_messages_start: self.current_turn_messages_start,
                compactions: self.compactions.clone(),
                history_replacement_len: self.history_replacement_len,
            },
        });
        Ok(())
    }

    fn reject_preparation(&mut self) {
        if let Some(pending) = self.pending_preparation.take() {
            let previous = pending.previous;
            self.model_messages = previous.model_messages;
            self.persistent_messages = previous.persistent_messages;
            self.current_user_message_id = previous.current_user_message_id;
            self.current_turn_messages_start = previous.current_turn_messages_start;
            self.compactions = previous.compactions;
            self.history_replacement_len = previous.history_replacement_len;
        }
    }

    pub(super) fn accept_checkpoint(
        &mut self,
        host: &WorkflowModuleHostMut<'_>,
        checkpoint: WorkflowHistoryCheckpoint,
    ) -> Result<(), ProcessModuleError> {
        let result = (|| {
            let json = host.checkpoint_history_json(
                serde_json::to_string(&checkpoint)
                    .map_err(|error| ProcessModuleError::new(error.to_string()))?,
            )?;
            let ack: WorkflowCheckpointAck = serde_json::from_str(&json)
                .map_err(|error| ProcessModuleError::new(error.to_string()))?;
            if ack.revision.is_some() {
                if current_user_index(&ack.messages, self.current_user_message_id).is_none() {
                    return Err(ProcessModuleError::new(
                        "checkpoint acknowledgement dropped the current user message",
                    ));
                }
                self.retire_acknowledged_results(&ack.messages)?;
                self.model_messages = merge_request_context(&self.model_messages, &ack.messages);
                self.persistent_messages = ack.messages;
                self.compactions = ack.history.compactions;
                self.history_replacement_len =
                    ack.history.history_replacement.as_ref().map(Vec::len);
                self.current_turn_messages_start =
                    current_user_index(&self.model_messages, self.current_user_message_id).unwrap()
                        + 1;
            }
            // A detached acknowledgement carries a partial projection. Keep the
            // workflow's full initial history rather than substituting ack.messages.
            self.pending_preparation = None;
            Ok(())
        })();
        if result.is_err() {
            self.reject_preparation();
        }
        result
    }
}

/// Keep request-only context at its next retained canonical anchor while
/// adopting the host's accepted history, including newly delivered steering.
fn merge_request_context(
    model: &[CanonicalMessage],
    canonical: &[CanonicalMessage],
) -> Vec<CanonicalMessage> {
    use proteus_contracts::model_standard::PartScope;
    let ephemeral = |message: &CanonicalMessage| {
        !message.parts.is_empty()
            && message
                .parts
                .iter()
                .all(|part| part.scope == PartScope::Request)
    };
    let mut groups = std::collections::HashMap::<MessageId, Vec<CanonicalMessage>>::new();
    let mut trailing = Vec::new();
    for (index, message) in model
        .iter()
        .enumerate()
        .filter(|(_, message)| ephemeral(message))
    {
        let next = model[index + 1..].iter().find(|next| {
            !ephemeral(next) && canonical.iter().any(|accepted| accepted.id == next.id)
        });
        match next {
            Some(next) => groups.entry(next.id).or_default().push(message.clone()),
            None => trailing.push(message.clone()),
        }
    }
    let mut merged = Vec::with_capacity(model.len().max(canonical.len()));
    for message in canonical {
        merged.extend(groups.remove(&message.id).unwrap_or_default());
        merged.push(message.clone());
    }
    merged.extend(trailing);
    merged
}
