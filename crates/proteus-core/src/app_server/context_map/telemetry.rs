use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
    sync::Arc,
};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;

use crate::{
    app_server::{
        AppContextBuildSnapshot, AppContextCompactionSnapshot, AppContextToolSummary,
        AppContextUsageCategory, AppContextUsageSnapshot, paths_equal,
    },
    domain::{Event, EventEnvelope, SessionId, TokenUsageSource},
};

mod cache;

/// Only log-derived state is reusable: callers supply current history and activity.
#[derive(Default, Serialize)]
pub(super) struct Telemetry {
    pub tools: AppContextToolSummary,
    pub latest_usage: Option<AppContextUsageSnapshot>,
    pub latest_context: Option<AppContextBuildSnapshot>,
    pub latest_compaction: Option<AppContextCompactionSnapshot>,
    pub diagnostics: Vec<String>,
}

pub(super) fn read(
    path: &Path,
    session_id: Option<SessionId>,
    session_dir: Option<&Path>,
) -> Result<Arc<Telemetry>> {
    let before = cache::SourceStamp::read(path);
    let key = cache::Key::new(path, session_id, session_dir);
    if let Some(source) = before.as_ref()
        && let Some(telemetry) = cache::get(&key, source)
    {
        return Ok(telemetry);
    }
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Arc::new(Telemetry {
                diagnostics: vec!["event log not found; using session history only".to_owned()],
                ..Default::default()
            }));
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let opened = file
        .metadata()
        .ok()
        .and_then(cache::SourceStamp::from_metadata);
    let telemetry = Arc::new(
        parse(BufReader::new(file), session_id, session_dir)
            .with_context(|| format!("failed to read {}", path.display()))?,
    );
    // Never label a mixed/replaced read as belonging to a newer source version.
    if let Some(before) = before
        && opened.as_ref() == Some(&before)
        && cache::SourceStamp::read(path).as_ref() == Some(&before)
    {
        cache::insert(key, before, telemetry.clone());
    }
    Ok(telemetry)
}

fn parse(
    mut reader: impl BufRead,
    session_id: Option<SessionId>,
    session_dir: Option<&Path>,
) -> Result<Telemetry> {
    let mut telemetry = Telemetry::default();
    let mut names = BTreeSet::new();
    let mut skipped = 0usize;
    let mut line = String::new();
    while reader.read_line(&mut line)? != 0 {
        if !line.trim().is_empty() {
            match serde_json::from_str::<EventEnvelope>(&line) {
                Ok(envelope) if matches_session(&envelope, session_id, session_dir) => {
                    telemetry.apply(envelope, &mut names);
                }
                Ok(_) => {}
                Err(_) => skipped += 1,
            }
        }
        line.clear();
    }
    telemetry.tools.names = names.into_iter().collect();
    if skipped > 0 {
        telemetry
            .diagnostics
            .push(format!("skipped {skipped} malformed event log lines"));
    }
    Ok(telemetry)
}

fn matches_session(
    envelope: &EventEnvelope,
    session_id: Option<SessionId>,
    session_dir: Option<&Path>,
) -> bool {
    if let Some(session_id) = session_id {
        return envelope.session_id == session_id;
    }
    match (&envelope.event, session_dir) {
        (
            Event::SessionStarted {
                session_dir: Some(started),
                ..
            },
            Some(directory),
        ) => paths_equal(started, directory),
        _ => false,
    }
}

impl Telemetry {
    fn apply(&mut self, envelope: EventEnvelope, names: &mut BTreeSet<String>) {
        match envelope.event {
            Event::ToolCallRequested { call } => {
                self.tools.requested += 1;
                names.insert(call.name);
            }
            Event::ToolFinished { result } => {
                self.tools.finished += 1;
                if !result.ok {
                    self.tools.failed += 1;
                }
            }
            Event::ContextBuilt {
                chunks,
                token_estimate,
            } => {
                let mut context = AppContextBuildSnapshot::new(chunks);
                context.token_estimate = token_estimate;
                context.turn_id = envelope.turn_id;
                context.timestamp_ms = Some(envelope.timestamp_ms);
                self.latest_context = Some(context);
            }
            Event::TokenUsageUpdated { usage } => {
                let mut snapshot = AppContextUsageSnapshot::new(
                    usage.model.provider,
                    usage.model.model,
                    usage.estimated_input_tokens,
                    source_label(usage.source),
                );
                snapshot.phase = usage.phase;
                snapshot.max_input_tokens = usage.max_input_tokens;
                snapshot.compaction_trigger_tokens = usage.compaction_trigger_tokens;
                snapshot.categories = usage
                    .categories
                    .into_iter()
                    .map(|category| {
                        AppContextUsageCategory::new(category.name, category.tokens)
                            .with_source(source_label(category.source))
                    })
                    .collect();
                snapshot.actual = usage.actual;
                snapshot.turn_id = envelope.turn_id;
                snapshot.timestamp_ms = Some(envelope.timestamp_ms);
                self.latest_usage = Some(snapshot);
            }
            Event::HistoryCompactionStarted { .. } => {
                self.compaction("started", envelope.turn_id, envelope.timestamp_ms);
            }
            Event::HistoryCompactionCompleted { mut report } => {
                let summary_present = report.summary.is_some();
                report.summary = None;
                report.metadata = Value::Null;
                self.compaction("completed", envelope.turn_id, envelope.timestamp_ms);
                let snapshot = self.latest_compaction.as_mut().unwrap();
                snapshot.report = Some(report);
                snapshot.summary_present = summary_present;
            }
            Event::HistoryCompactionFailed { .. } => {
                self.compaction("failed", envelope.turn_id, envelope.timestamp_ms);
            }
            _ => {}
        }
    }

    fn compaction(&mut self, status: &str, turn_id: Option<crate::domain::TurnId>, timestamp: i64) {
        let mut snapshot = AppContextCompactionSnapshot::new(status);
        snapshot.turn_id = turn_id;
        snapshot.timestamp_ms = Some(timestamp);
        self.latest_compaction = Some(snapshot);
    }
}

fn source_label(source: TokenUsageSource) -> &'static str {
    match source {
        TokenUsageSource::Estimated => "estimated",
        TokenUsageSource::Provider => "provider",
        TokenUsageSource::Mixed => "mixed",
        _ => "unknown",
    }
}
