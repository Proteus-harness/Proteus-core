use std::collections::HashMap;

use super::{AppTranscriptMessage, AppTranscriptTool};

/// One render's append-only rows and the last card for each call identity.
/// Neither result/resolution nor citation/image updates change call IDs or row
/// positions. A display reset must clear both rows and their lookup together.
#[derive(Default)]
pub(super) struct TranscriptRows {
    pub(super) rows: Vec<AppTranscriptMessage>,
    latest_tools: HashMap<String, usize>,
    reasoning_rows: HashMap<crate::domain::MessageId, usize>,
}

#[cfg(test)]
mod tests;

impl TranscriptRows {
    pub(super) fn push(&mut self, row: AppTranscriptMessage) {
        if row.role == "reasoning"
            && let Some(id) = row.message_id
        {
            self.reasoning_rows.insert(id, self.rows.len());
        }
        if let Some(tool) = &row.tool {
            self.latest_tools
                .insert(tool.call_id.clone(), self.rows.len());
        }
        self.rows.push(row);
    }

    pub(super) fn clear(&mut self) {
        self.rows.clear();
        self.latest_tools.clear();
        self.reasoning_rows.clear();
    }

    /// Journal part de-duplication happens before this method. Multiple readable
    /// parts of one canonical item share one separate display row.
    pub(super) fn append_reasoning(&mut self, message_id: crate::domain::MessageId, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if let Some(index) = self.reasoning_rows.get(&message_id) {
            let row = &mut self.rows[*index];
            row.text.push('\n');
            row.text.push_str(text);
            return;
        }
        self.push(AppTranscriptMessage {
            message_id: Some(message_id),
            timestamp_ms: None,
            phase: None,
            role: "reasoning".to_owned(),
            text: text.to_owned(),
            images: Vec::new(),
            tool: None,
            subagent: None,
            streaming: false,
        });
    }

    pub(super) fn tool_mut(&mut self, call_id: &str) -> Option<&mut AppTranscriptTool> {
        let index = *self.latest_tools.get(call_id)?;
        self.rows[index].tool.as_mut()
    }
}
