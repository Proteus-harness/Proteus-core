//! Uniqueness within the current conversation (not all parts ever observed).
#[cfg(test)]
mod tests;

use super::*;
use proteus_contracts::domain::MessageId;
use std::collections::HashSet;

#[derive(Clone, Debug, Default)]
pub(super) struct ActiveIds {
    messages: HashSet<MessageId>,
    parts: HashSet<PartId>,
}

impl JournalValidationState {
    pub(super) fn check_active_ids(&mut self, appended: Option<usize>) -> Result<()> {
        // Take the index before checking. A rejected mutation can leave the
        // history invalid; the next operation must then check the whole history.
        let previous = self.active_ids.take();
        let (mut ids, rows) = match (appended, previous) {
            (Some(count), Some(ids)) => (ids, &self.history[self.history.len() - count..]),
            _ => (Arc::default(), self.history.as_slice()),
        };
        Arc::make_mut(&mut ids).extend(rows)?;
        self.active_ids = Some(ids);
        Ok(())
    }
}

impl ActiveIds {
    fn extend(&mut self, messages: &[CanonicalMessage]) -> Result<()> {
        for message in messages {
            if !self.messages.insert(message.id) {
                bail!(
                    "active history contains duplicate message id {}",
                    message.id
                );
            }
            for part in &message.parts {
                if !self.parts.insert(part.part_id) {
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
}
