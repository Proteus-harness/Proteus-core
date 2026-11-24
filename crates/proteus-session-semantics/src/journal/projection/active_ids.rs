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
    pub(super) fn check_captured_active_ids(&mut self, index: usize) -> Result<()> {
        // A collision needs the original ordered scan to report its exact row.
        // An aliased index must not be mutated or copied for this fast path.
        if let Some(message) = self.history.get(index)
            && let [part] = message.parts.as_slice()
            && let Some(ids) = self.active_ids.as_mut().and_then(Arc::get_mut)
            && !ids.messages.contains(&message.id)
            && !ids.parts.contains(&part.part_id)
        {
            ids.messages.insert(message.id);
            ids.parts.insert(part.part_id);
            return Ok(());
        }
        self.check_active_ids(None)
    }

    /// Same row order as checking a fully materialized append. The streaming
    /// fold may validate the borrowed tail before transferring its owners.
    pub(super) fn check_owned_append_ids(&mut self, appended: &[CanonicalMessage]) -> Result<()> {
        let mut ids = match self.active_ids.take() {
            Some(ids) => ids,
            None => {
                let mut ids = ActiveIds::default();
                ids.extend(&self.history)?;
                Arc::new(ids)
            }
        };
        Arc::make_mut(&mut ids).extend(appended)?;
        self.active_ids = Some(ids);
        Ok(())
    }

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
