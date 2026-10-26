use std::path::PathBuf;

use anyhow::Result;

use crate::{domain::SessionId, model_standard::CanonicalMessage};

use super::{AppContextHistorySummary, AppContextMapSnapshot, AppSessionActivity};

use crate::core::summarize_context_history;

mod telemetry;
#[cfg(test)]
mod tests;

pub(super) struct ContextMapInput {
    pub session_dir: Option<PathBuf>,
    pub session_id: Option<SessionId>,
    pub workspace_path: Option<PathBuf>,
    pub activity: Option<AppSessionActivity>,
    pub history: Vec<CanonicalMessage>,
    pub event_log_path: PathBuf,
    pub diagnostics: Vec<String>,
}

pub(super) fn build_context_map_snapshot(input: ContextMapInput) -> Result<AppContextMapSnapshot> {
    build_context_map_snapshot_with_summary(input, None)
}

pub(super) fn build_context_map_snapshot_with_summary(
    input: ContextMapInput,
    summary: Option<AppContextHistorySummary>,
) -> Result<AppContextMapSnapshot> {
    let mut diagnostics = input.diagnostics;
    let telemetry = telemetry::read(
        &input.event_log_path,
        input.session_id,
        input.session_dir.as_deref(),
    )?;
    let history = summary.unwrap_or_else(|| summarize_context_history(&input.history));
    diagnostics.extend(telemetry.diagnostics.iter().cloned());
    let mut snapshot = AppContextMapSnapshot::new(
        input.session_dir,
        input.session_id,
        input.workspace_path,
        history,
        telemetry.tools.clone(),
    );
    snapshot.activity = input.activity;
    snapshot.latest_context = telemetry.latest_context.clone();
    snapshot.latest_usage = telemetry.latest_usage.clone();
    snapshot.latest_compaction = telemetry.latest_compaction.clone();
    if snapshot.latest_usage.is_none() {
        diagnostics
            .push("token usage telemetry unavailable; showing history-only fallback".to_owned());
    }
    snapshot.diagnostics = diagnostics;
    Ok(snapshot)
}
