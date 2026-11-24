//! Identical envelope/order checks for collected projections and streaming recovery.
use super::*;

impl JournalValidationState {
    pub fn apply_sequenced(
        &mut self,
        session_id: SessionId,
        record: &JournalRecord,
        expected_seq: &mut u64,
    ) -> Result<()> {
        check_envelope(session_id, record, *expected_seq)?;
        self.apply(record)?;
        *expected_seq = expected_seq.saturating_add(1);
        Ok(())
    }

    /// Fold an owned journal record in a temporary recovery pass. The observer
    /// receives the original record only after full validation, and before any
    /// owned append or replacement messages are moved. Discard this state on error, just as
    /// when building a collected projection.
    pub fn apply_sequenced_for_read(
        &mut self,
        session_id: SessionId,
        record: JournalRecord,
        expected_seq: &mut u64,
        observe: impl FnOnce(&JournalRecord),
    ) -> Result<()> {
        check_envelope(session_id, &record, *expected_seq)?;
        if let JournalEntry::HistoryMutated(mutation) = &record.entry
            && matches!(
                mutation.mutation,
                HistoryMutationKind::Append | HistoryMutationKind::Replace
            )
        {
            self.require_new_record_id(&record)?;
            self.validate_history_mutation(&record, mutation)?;
            let replace = mutation.mutation == HistoryMutationKind::Replace;
            if replace {
                self.prepare_history_replacement(mutation)?;
                // The previous conversation is no longer a consumer input.
                // Drop this recovery owner's reference before indexing or
                // staging the replacement, as on the borrowed path.
                self.history = Arc::default();
                self.check_replacement_ids(&mutation.messages)?;
            } else {
                super::facts::validate_history_append(mutation)?;
                self.check_owned_append_ids(&mutation.messages)?;
            }
            self.finish_history_mutation(&record, mutation);
            observe(&record);
            let JournalEntry::HistoryMutated(mutation) = record.entry else {
                unreachable!("validated history mutation");
            };
            if replace || self.history.is_empty() {
                self.history = Arc::new(mutation.messages);
            } else {
                Arc::make_mut(&mut self.history).extend(mutation.messages);
            }
        } else {
            self.apply(&record)?;
            observe(&record);
        }
        *expected_seq = expected_seq.saturating_add(1);
        Ok(())
    }
}

fn check_envelope(session_id: SessionId, record: &JournalRecord, expected_seq: u64) -> Result<()> {
    if record.schema_version != JOURNAL_SCHEMA_VERSION {
        bail!(
            "unsupported journal schema_version {} in record {}; expected {}",
            record.schema_version,
            record.record_id,
            JOURNAL_SCHEMA_VERSION
        );
    }
    if record.session_id != session_id {
        bail!(
            "journal record {} belongs to session {}, expected {}",
            record.record_id,
            record.session_id,
            session_id
        );
    }
    if record.session_seq != expected_seq {
        bail!(
            "journal sequence mismatch: expected {}, found {}",
            expected_seq,
            record.session_seq
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
