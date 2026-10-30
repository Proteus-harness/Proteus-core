//! Strict derived-only reads without retaining a second copy of all raw records.
use super::*;
use crate::{
    contracts::ConversationSnapshot,
    core::{model_context::projection::ModelContextProjection, session_journal::UsageProjection},
    domain::SessionUsageSnapshot,
    model_standard::CanonicalMessage,
};

pub(crate) fn load_validated_history(
    session_dir: &Path,
    session_id: SessionId,
) -> Result<Vec<CanonicalMessage>> {
    Ok(scan_validated(session_dir, session_id, |_| {})?.into_history())
}

pub(crate) fn load_validated_history_revision(
    session_dir: &Path,
    session_id: SessionId,
) -> Result<u64> {
    Ok(scan_validated(session_dir, session_id, |_| {})?.history_revision())
}

pub(crate) fn load_validated_conversation(
    session_dir: &Path,
    session_id: SessionId,
) -> Result<ConversationSnapshot> {
    let mut context = ModelContextProjection::default();
    let state = scan_validated(session_dir, session_id, |record| context.apply(record))?;
    Ok(ConversationSnapshot {
        revision: state.history_revision(),
        messages: state.into_history(),
        model_context: context.snapshot(),
    })
}

pub(crate) fn load_validated_usage(
    session_dir: &Path,
    session_id: SessionId,
) -> Result<SessionUsageSnapshot> {
    let mut usage = UsageProjection::default();
    scan_validated(session_dir, session_id, |record| usage.apply(record))?;
    Ok(usage.snapshot(session_id))
}

// This private fold is used only by the fixed, local metadata projections above.
// It is not an early-render hook and never receives a caller's callback.
fn scan_validated(
    session_dir: &Path,
    session_id: SessionId,
    mut fold: impl FnMut(&JournalRecord),
) -> Result<JournalValidationState> {
    let mut state = JournalValidationState::default();
    let mut next_seq = 1;
    let mut semantic_error = None;
    read::scan_records(session_dir, session_id, |record| {
        if semantic_error.is_none() {
            match state.apply_sequenced(session_id, &record, &mut next_seq) {
                Ok(()) => fold(&record),
                Err(error) => semantic_error = Some(error),
            }
        }
        // load_records + JournalProjection::build decodes the entire complete
        // prefix before semantic validation. Keep later parse/blob failures
        // ahead of an earlier semantic error, without retaining the records.
        Ok(())
    })?;
    if let Some(error) = semantic_error {
        return Err(error);
    }
    Ok(state)
}

#[cfg(test)]
mod tests;
