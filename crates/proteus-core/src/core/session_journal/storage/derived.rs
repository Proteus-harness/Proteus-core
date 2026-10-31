//! Strict derived-only reads without retaining a second copy of all raw records.
use super::*;
use crate::{
    contracts::ConversationSnapshot,
    core::{
        model_context::{ModelContextState, projection::ModelContextProjection},
        session_journal::UsageProjection,
    },
    domain::{SessionUsageSnapshot, TurnId},
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

/// Execution state without raw historical request, hook or checkpoint payloads.
#[derive(Debug)]
pub(crate) struct JournalResumeState {
    pub history: Vec<CanonicalMessage>,
    pub history_revision: u64,
    pub interrupted_turns: Vec<crate::contracts::WorkflowHistoryInterruption>,
    pub model_context: ModelContextState,
    pub usage: SessionUsageSnapshot,
    pub last_history_turn: Option<TurnId>,
}

/// Strict validation completes before any consumer can render the fixed prefix.
#[derive(Debug)]
pub(crate) struct ValidatedJournalRead {
    snapshot: read::JournalReadSnapshot,
    validation: JournalValidationState,
    context: ModelContextProjection,
    usage: UsageProjection,
    session_id: SessionId,
}

impl ValidatedJournalRead {
    pub(crate) fn open(session_dir: &Path, session_id: SessionId) -> Result<Self> {
        let mut snapshot = read::JournalReadSnapshot::open(session_dir, session_id)?;
        let mut context = ModelContextProjection::default();
        let mut usage = UsageProjection::default();
        let validation = scan_validated_snapshot(&mut snapshot, session_id, |record| {
            context.apply(record);
            usage.apply(record);
        })?;
        Ok(Self {
            snapshot,
            validation,
            context,
            usage,
            session_id,
        })
    }

    pub(crate) fn history(&self) -> &[CanonicalMessage] {
        self.validation.history()
    }
    pub(crate) fn history_revision(&self) -> u64 {
        self.validation.history_revision()
    }
    pub(crate) fn interrupted_turns(&self) -> Vec<crate::contracts::WorkflowHistoryInterruption> {
        self.validation.interrupted_turns()
    }
    pub(crate) fn last_history_turn(&self) -> Option<TurnId> {
        self.validation.last_history_turn()
    }
    pub(crate) fn model_context(&self) -> Vec<crate::contracts::ModelContextObservation> {
        self.context.snapshot()
    }
    pub(crate) fn usage(&self) -> SessionUsageSnapshot {
        self.usage.snapshot(self.session_id)
    }

    pub(crate) fn into_parts(self) -> (read::JournalReadSnapshot, JournalResumeState) {
        let resume = JournalResumeState {
            history_revision: self.validation.history_revision(),
            interrupted_turns: self.validation.interrupted_turns(),
            model_context: ModelContextState::from_snapshot(self.context.snapshot()),
            usage: self.usage.snapshot(self.session_id),
            last_history_turn: self.validation.last_history_turn(),
            history: self.validation.into_history(),
        };
        (self.snapshot, resume)
    }

    pub(crate) fn into_resume_state(self) -> JournalResumeState {
        self.into_parts().1
    }
}

// This private fold is used only by fixed local projections, never by an
// arbitrary renderer. Hydration errors take precedence over semantic errors.
fn scan_validated(
    session_dir: &Path,
    session_id: SessionId,
    fold: impl FnMut(&JournalRecord),
) -> Result<JournalValidationState> {
    let mut snapshot = read::JournalReadSnapshot::open(session_dir, session_id)?;
    scan_validated_snapshot(&mut snapshot, session_id, fold)
}

fn scan_validated_snapshot(
    snapshot: &mut read::JournalReadSnapshot,
    session_id: SessionId,
    mut fold: impl FnMut(&JournalRecord),
) -> Result<JournalValidationState> {
    #[cfg(feature = "memory-diagnostics")]
    let memory = crate::memory_diagnostics::Scope::new("journal.validate_stream");
    let mut state = JournalValidationState::default();
    let mut next_seq = 1;
    let mut semantic_error = None;
    snapshot.scan(|record| {
        if semantic_error.is_none() {
            match state.apply_sequenced(session_id, &record, &mut next_seq) {
                Ok(()) => fold(&record),
                Err(error) => semantic_error = Some(error),
            }
        }
        // Match load_records + JournalProjection::build: late storage failures
        // still win over an earlier semantic error, without retaining records.
        Ok(())
    })?;
    if let Some(error) = semantic_error {
        return Err(error);
    }
    #[cfg(feature = "memory-diagnostics")]
    memory.sample(
        "journal.stream_validated",
        &[("history", state.history().len())],
    );
    Ok(state)
}

#[cfg(test)]
mod tests;
