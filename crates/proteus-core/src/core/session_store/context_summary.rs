//! Compact context statistics shared by live reads and the SQLite display index.
use crate::model_standard::{CanonicalMessage, ContentPart, MessageRole};
use proteus_contracts::app_protocol::AppContextHistorySummary;

pub(crate) fn summarize_context_history(messages: &[CanonicalMessage]) -> AppContextHistorySummary {
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
