//! Recover the sole writer without keeping obsolete journal payloads alive.
use super::*;

pub(crate) fn initialize_writer_state(
    session_dir: &Path,
    session_id: SessionId,
    state: &mut JournalWriterState,
) -> Result<()> {
    initialize(session_dir, session_id, state, false).map(|_| ())
}

pub(crate) fn initialize_writer_projection(
    session_dir: &Path,
    session_id: SessionId,
    state: &mut JournalWriterState,
) -> Result<Option<JournalProjection>> {
    initialize(session_dir, session_id, state, true)
}

fn initialize(
    session_dir: &Path,
    session_id: SessionId,
    state: &mut JournalWriterState,
    include_projection: bool,
) -> Result<Option<JournalProjection>> {
    state.ensure_active()?;
    if state.initialized {
        return Ok(None);
    }
    state.with_ownership_preflight(session_dir, session_id, |state| {
        recover(session_dir, session_id, state, include_projection)
    })
}

fn recover(
    session_dir: &Path,
    session_id: SessionId,
    state: &mut JournalWriterState,
    include_projection: bool,
) -> Result<Option<JournalProjection>> {
    #[cfg(feature = "memory-diagnostics")]
    let memory = crate::memory_diagnostics::Scope::new("writer.recover");
    #[cfg(feature = "memory-diagnostics")]
    memory.sample(
        "writer.recovery_started",
        &[("projection_requested", usize::from(include_projection))],
    );
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
    // Resume/replay callers explicitly need records. An ordinary cold append
    // only needs the resulting validated indices, history and compact metadata.
    let mut records = include_projection.then(Vec::new);
    read::scan_records(session_dir, session_id, |record| {
        validation.apply_sequenced(session_id, &record, &mut next_seq)?;
        usage.apply(&record);
        model_context.apply(&record);
        if let Some(records) = &mut records {
            records.push(record);
        }
        Ok(())
    })?;
    #[cfg(feature = "memory-diagnostics")]
    memory.sample(
        "writer.recovery_scan_complete",
        &[
            ("records", records.as_ref().map_or(0, Vec::len)),
            ("history", validation.history().len()),
        ],
    );
    let committed_offset = journal_len(&path)?;
    #[cfg(feature = "memory-diagnostics")]
    memory.sample(
        "writer.before_validation_clone",
        &[("history", validation.history().len())],
    );
    let projection =
        records.map(|records| JournalProjection::from_validated(records, validation.clone()));
    #[cfg(feature = "memory-diagnostics")]
    memory.sample(
        "writer.after_projection_build",
        &[("history", validation.history().len())],
    );
    state.next_seq = next_seq;
    state.validation = validation;
    state.usage = usage;
    state.model_context = model_context;
    state.committed_offset = committed_offset;
    state.initialized = true;
    Ok(projection)
}
