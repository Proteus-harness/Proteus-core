//! Compact context statistics shared by live reads and the SQLite display index.
use std::{fmt, path::Path};

use crate::model_standard::{CanonicalMessage, ContentPart, MessageRole};
use proteus_contracts::app_protocol::AppContextHistorySummary;

fn json_bytes(value: &impl serde::Serialize) -> usize {
    super::json_size::encoded_len(value, usize::MAX).unwrap_or_default()
}

fn path_display_bytes(path: &Path) -> usize {
    struct ByteCount(usize);

    impl fmt::Write for ByteCount {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            self.0 += text.len();
            Ok(())
        }
    }

    let mut count = ByteCount(0);
    fmt::write(&mut count, format_args!("{}", path.display()))
        .expect("counting path display bytes cannot fail");
    count.0
}

pub(crate) fn summarize_context_history(messages: &[CanonicalMessage]) -> AppContextHistorySummary {
    let mut summary = AppContextHistorySummary::default();
    summary.messages = messages.len();
    let mut bytes = 0usize;
    for message in messages {
        bytes += json_bytes(&message.metadata) + 4;
        note_role(&mut summary, &message.role);
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
                            .map(|path| path_display_bytes(path))
                            .unwrap_or_default()
                        + chunk.content.len()
                        + json_bytes(&chunk.metadata);
                }
                ContentPart::FileRef { path, content } => {
                    bytes +=
                        path_display_bytes(path) + content.as_deref().unwrap_or_default().len();
                }
                ContentPart::ToolCall { call } => {
                    bytes += call.name.len() + json_bytes(&call.args);
                }
                ContentPart::ToolResult { result } => {
                    summary.tool_results += 1;
                    bytes += result.output.len()
                        + result.error.as_deref().unwrap_or_default().len()
                        + json_bytes(&result.metadata)
                        + result.content.iter().map(json_bytes).sum::<usize>();
                }
                ContentPart::Patch { patch } => {
                    bytes += patch.content.len();
                }
                ContentPart::HostedToolActivity { activity } => {
                    bytes += json_bytes(activity);
                }
                ContentPart::Citation { citation } => {
                    bytes += json_bytes(citation);
                }
                _ => {}
            }
        }
    }
    summary.estimated_tokens = estimate_tokens_from_bytes(bytes);
    summary
}

pub(super) fn summarize_plain_history<'a>(
    messages: impl ExactSizeIterator<Item = (&'a MessageRole, &'a str)>,
) -> AppContextHistorySummary {
    let mut summary = AppContextHistorySummary::default();
    summary.messages = messages.len();
    let fixed = json_bytes(&serde_json::Value::Null) + 4;
    let mut bytes = 0usize;
    for (role, text) in messages {
        note_role(&mut summary, role);
        bytes += fixed + text.len();
    }
    summary.estimated_tokens = estimate_tokens_from_bytes(bytes);
    summary
}

fn note_role(summary: &mut AppContextHistorySummary, role: &MessageRole) {
    match role {
        MessageRole::User => summary.user_messages += 1,
        MessageRole::Assistant => summary.assistant_messages += 1,
        MessageRole::System | MessageRole::Developer => summary.system_messages += 1,
        MessageRole::Tool => {}
        _ => {}
    }
}

fn estimate_tokens_from_bytes(bytes: usize) -> u32 {
    if bytes == 0 {
        0
    } else {
        u32::try_from((bytes / 4).max(1)).unwrap_or(u32::MAX)
    }
}
