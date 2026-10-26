use std::path::PathBuf;

use anyhow::Result;

use crate::{
    domain::SessionId,
    model_standard::{CanonicalMessage, ContentPart, MessageRole},
};

use super::{AppContextHistorySummary, AppContextMapSnapshot, AppSessionActivity};

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
    let mut diagnostics = input.diagnostics;
    let telemetry = telemetry::read(
        &input.event_log_path,
        input.session_id,
        input.session_dir.as_deref(),
    )?;
    let history = summarize_context_history(&input.history);
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

fn summarize_context_history(messages: &[CanonicalMessage]) -> AppContextHistorySummary {
    let mut summary = AppContextHistorySummary::default();
    summary.messages = messages.len();
    let mut bytes = 0usize;
    for message in messages {
        bytes += message.metadata.to_string().len() + 4;
        match message.role {
            MessageRole::User => summary.user_messages += 1,
            MessageRole::Assistant => summary.assistant_messages += 1,
            MessageRole::System | MessageRole::Developer => summary.system_messages += 1,
            MessageRole::Tool => {}
            _ => {}
        }
        for part in &message.parts {
            match &part.payload {
                ContentPart::Text { text }
                | ContentPart::ReasoningSummary { text }
                | ContentPart::Reasoning { text, .. } => {
                    bytes += text.len();
                }
                ContentPart::Context { chunk } => {
                    bytes += chunk.source.len()
                        + chunk
                            .path
                            .as_ref()
                            .map(|path| path.display().to_string().len())
                            .unwrap_or_default()
                        + chunk.content.len()
                        + chunk.metadata.to_string().len();
                }
                ContentPart::FileRef { path, content } => {
                    bytes += path.display().to_string().len()
                        + content.as_deref().unwrap_or_default().len();
                }
                ContentPart::ToolCall { call } => {
                    bytes += call.name.len() + call.args.to_string().len();
                }
                ContentPart::ToolResult { result } => {
                    summary.tool_results += 1;
                    bytes += result.output.len()
                        + result.error.as_deref().unwrap_or_default().len()
                        + result.metadata.to_string().len()
                        + result
                            .content
                            .iter()
                            .map(|content| {
                                serde_json::to_string(content)
                                    .map(|text| text.len())
                                    .unwrap_or(0)
                            })
                            .sum::<usize>();
                }
                ContentPart::Patch { patch } => {
                    bytes += patch.content.len();
                }
                ContentPart::HostedToolActivity { activity } => {
                    bytes += serde_json::to_string(activity)
                        .map(|value| value.len())
                        .unwrap_or_default();
                }
                ContentPart::Citation { citation } => {
                    bytes += serde_json::to_string(citation)
                        .map(|value| value.len())
                        .unwrap_or_default();
                }
                _ => {}
            }
        }
    }
    summary.estimated_tokens = estimate_tokens_from_bytes(bytes);
    summary
}

fn estimate_tokens_from_bytes(bytes: usize) -> u32 {
    if bytes == 0 {
        0
    } else {
        u32::try_from((bytes / 4).max(1)).unwrap_or(u32::MAX)
    }
}
