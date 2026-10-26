//! Identical envelope/order checks for collected projections and streaming recovery.
use super::*;

impl JournalValidationState {
    pub(crate) fn apply_sequenced(
        &mut self,
        session_id: SessionId,
        record: &JournalRecord,
        expected_seq: &mut u64,
    ) -> Result<()> {
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
        if record.session_seq != *expected_seq {
            bail!(
                "journal sequence mismatch: expected {}, found {}",
                expected_seq,
                record.session_seq
            );
        }
        self.apply(record)?;
        *expected_seq = expected_seq.saturating_add(1);
        Ok(())
    }
}
