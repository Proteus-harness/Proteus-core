use std::collections::{HashMap, HashSet};

use anyhow::{Result, ensure};

use crate::{
    contracts::WorkflowToolResultBinding,
    domain::{ToolCall, ToolResult},
    model_standard::{CanonicalMessage, ContentPart},
};

/// An explicitly declared ordered result suffix. Completion order may differ
/// from call order; stable identities and insertion positions preserve both
/// ordinary workflow output and cold projection.
#[derive(Debug, Clone, Default)]
pub(crate) struct HistoryCapture {
    base: usize,
    calls: Vec<(ToolCall, WorkflowToolResultBinding, bool)>,
}

impl HistoryCapture {
    pub(crate) fn contains_call(&self, call_id: &str) -> bool {
        self.calls
            .iter()
            .any(|(_, binding, _)| binding.call_id == call_id)
    }

    pub(crate) fn has_binding(&self, binding: &WorkflowToolResultBinding) -> bool {
        self.calls
            .iter()
            .any(|(_, existing, _)| existing == binding)
    }

    /// Extend the model prefix while retaining the already committed result
    /// suffix. Only an identical binding may carry a result across checkpoints.
    /// This is shared by live history and cold journal projection.
    pub(crate) fn rebase(
        &self,
        current: &[CanonicalMessage],
        proposed: &[CanonicalMessage],
        bindings: &[WorkflowToolResultBinding],
        compacted: bool,
    ) -> Result<(Self, Vec<CanonicalMessage>)> {
        let mut next = Self::new(proposed, bindings)?;
        let mut history = proposed.to_vec();
        // new() has already rejected duplicate call IDs. Borrow this checkpoint's
        // bindings only; the ordered calls vector remains the result authority.
        let bindings_by_call = bindings
            .iter()
            .map(|binding| (binding.call_id.as_str(), binding))
            .collect::<HashMap<_, _>>();
        for (_, binding, _) in &self.calls {
            if let Some(reused) = bindings_by_call.get(binding.call_id.as_str()) {
                ensure!(
                    *reused == binding,
                    "checkpoint changed a pending tool result binding"
                );
            }
        }
        if compacted || proposed.starts_with(current) {
            return Ok((next, history));
        }
        let recorded = self
            .calls
            .iter()
            .filter(|(_, _, recorded)| *recorded)
            .count();
        ensure!(
            self.base + recorded == current.len() && proposed.starts_with(&current[..self.base]),
            "checkpoint discarded committed history without a new compaction"
        );
        let mut proposed_by_id = HashMap::new();
        for message in proposed {
            // Preserve iter().find()'s first match even for repeated message IDs.
            proposed_by_id.entry(message.id).or_insert(message);
        }
        for ((_, binding, _), message) in self
            .calls
            .iter()
            .filter(|(_, _, recorded)| *recorded)
            .zip(&current[self.base..])
        {
            // An earlier ACK can already have delivered some results to the
            // workflow while other calls settle before this checkpoint. Those
            // acknowledged messages are now part of the proposed prefix; only
            // the still-pending suffix needs to be carried over. Keep exact
            // canonical identities and payloads rather than synthesizing them.
            if let Some(accepted) = proposed_by_id.get(&message.id) {
                ensure!(
                    *accepted == message,
                    "checkpoint changed a committed tool result"
                );
                continue;
            }
            ensure!(
                bindings_by_call
                    .get(binding.call_id.as_str())
                    .is_some_and(|existing| *existing == binding),
                "checkpoint discarded a committed tool result"
            );
            let result = message
                .parts
                .iter()
                .find_map(|part| match &part.payload {
                    ContentPart::ToolResult { result } => Some(result),
                    _ => None,
                })
                .ok_or_else(|| anyhow::anyhow!("checkpoint result suffix was changed"))?;
            ensure!(
                &binding.message(result.clone()) == message,
                "checkpoint result suffix was changed"
            );
            next.record(&mut history, result)?;
        }
        Ok((next, history))
    }

    pub(crate) fn new(
        history: &[CanonicalMessage],
        bindings: &[WorkflowToolResultBinding],
    ) -> Result<Self> {
        if bindings.is_empty() {
            return Ok(Self {
                base: history.len(),
                calls: Vec::new(),
            });
        }
        let mut message_ids = history
            .iter()
            .map(|message| message.id)
            .collect::<HashSet<_>>();
        let mut part_ids = history
            .iter()
            .flat_map(|message| message.parts.iter().map(|part| part.part_id))
            .collect::<HashSet<_>>();
        let mut history_calls = HashMap::new();
        let mut history_results = HashSet::new();
        for part in history.iter().flat_map(|message| &message.parts) {
            match &part.payload {
                ContentPart::ToolCall { call } => {
                    // Presence alone would hide ambiguous calls. Only the second
                    // occurrence matters, so no count or payload copy is needed.
                    history_calls
                        .entry(call.id.as_str())
                        .and_modify(|ambiguous| *ambiguous = true)
                        .or_insert(false);
                }
                ContentPart::ToolResult { result } => {
                    history_results.insert(result.call_id.as_str());
                }
                _ => {}
            }
        }
        let mut seen = HashSet::new();
        let mut calls = Vec::new();
        for binding in bindings {
            ensure!(
                binding.execution_call.id == binding.call_id,
                "checkpoint execution call id disagrees with history binding"
            );
            ensure!(
                seen.insert(binding.call_id.clone()),
                "duplicate checkpoint tool binding {}",
                binding.call_id
            );
            ensure!(
                message_ids.insert(binding.message_id) && part_ids.insert(binding.part_id),
                "checkpoint result identities are not unique"
            );
            let ambiguous = history_calls.get(binding.call_id.as_str()).ok_or_else(|| {
                anyhow::anyhow!("checkpoint binding {} has no history call", binding.call_id)
            })?;
            ensure!(!*ambiguous, "checkpoint binding has ambiguous history call");
            ensure!(
                !history_results.contains(binding.call_id.as_str()),
                "checkpoint binding already has a history result"
            );
            calls.push((binding.execution_call.clone(), binding.clone(), false));
        }
        Ok(Self {
            base: history.len(),
            calls,
        })
    }

    pub(crate) fn validate_call(&self, call: &ToolCall) -> Result<()> {
        if let Some((expected, _, _)) = self
            .calls
            .iter()
            .find(|(expected, _, _)| expected.id == call.id)
        {
            ensure!(
                call == expected,
                "executed tool call disagrees with checkpoint execution binding"
            );
        }
        Ok(())
    }

    pub(crate) fn record(
        &mut self,
        history: &mut Vec<CanonicalMessage>,
        result: &ToolResult,
    ) -> Result<bool> {
        let Some(index) = self
            .calls
            .iter()
            .position(|(_, binding, _)| binding.call_id == result.call_id)
        else {
            return Ok(false);
        };
        let offset = self.calls[..index]
            .iter()
            .filter(|(_, _, recorded)| *recorded)
            .count();
        let (_, binding, recorded) = &mut self.calls[index];
        ensure!(!*recorded, "checkpoint received a duplicate tool result");
        ensure!(
            self.base + offset <= history.len(),
            "checkpoint history suffix was lost"
        );
        history.insert(self.base + offset, binding.message(result.clone()));
        *recorded = true;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
