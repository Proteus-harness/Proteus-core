//! Recover the sole writer without keeping obsolete journal payloads alive.
use super::*;

pub(crate) fn initialize_writer_state(
    session_dir: &Path,
    session_id: SessionId,
    state: &mut JournalWriterState,
) -> Result<()> {
    state.ensure_active()?;
    if state.initialized {
        return Ok(());
    }
    state.with_ownership_preflight(session_dir, session_id, |state| {
        recover(session_dir, session_id, state)
    })
}

pub(crate) fn initialize_writer_resume(
    session_dir: &Path,
    session_id: SessionId,
    state: &mut JournalWriterState,
) -> Result<JournalResumeState> {
    initialize_writer_state(session_dir, session_id, state)?;
    // The writer remains validation authority. Only its current history is
    // copied for the runtime owner; obsolete payloads never survive recovery.
    Ok(JournalResumeState {
        history: state.validation.history().to_vec(),
        history_revision: state.validation.history_revision(),
        interrupted_turns: state.validation.interrupted_turns(),
        model_context: crate::core::model_context::ModelContextState::from_snapshot(
            state.model_context.snapshot(),
        ),
        usage: state.usage.snapshot(session_id),
        last_history_turn: state.validation.last_history_turn(),
    })
}

fn recover(
    session_dir: &Path,
    session_id: SessionId,
    state: &mut JournalWriterState,
) -> Result<()> {
    #[cfg(feature = "memory-diagnostics")]
    let memory = crate::memory_diagnostics::Scope::new("writer.recover");
    #[cfg(feature = "memory-diagnostics")]
    memory.sample("writer.recovery_started", &[]);
    let path = journal_path(session_dir);
    recovery::repair_unterminated_tail(&path)?;
    #[cfg(test)]
    {
        state.initial_recovery_scans += 1;
    }
    let mut validation = JournalValidationState::default();
    let mut usage = super::super::UsageProjection::default();
    let mut model_context =
        crate::core::model_context::projection::ModelContextProjection::default();
    let mut next_seq = 1;
    // Writer recovery keeps its existing fail-fast semantic error policy.
    read::scan_records(session_dir, session_id, |record| {
        validation.apply_sequenced(session_id, &record, &mut next_seq)?;
        usage.apply(&record);
        model_context.apply(&record);
        Ok(())
    })?;
    #[cfg(feature = "memory-diagnostics")]
    memory.sample(
        "writer.recovery_scan_complete",
        &[("history", validation.history().len())],
    );
    let committed_offset = journal_len(&path)?;
    state.next_seq = next_seq;
    state.validation = validation;
    state.usage = usage;
    state.model_context = model_context;
    state.committed_offset = committed_offset;
    state.initialized = true;
    #[cfg(feature = "memory-diagnostics")]
    memory.sample(
        "writer.compact_state_installed",
        &[("history", state.validation.history().len())],
    );
    Ok(())
}
