use std::collections::HashMap;

use super::{AppTranscriptMessage, AppTranscriptTool};

/// One render's append-only rows and the last card for each call identity.
/// Neither result/resolution nor citation/image updates change call IDs or row
/// positions. A display reset must clear both rows and their lookup together.
#[derive(Default)]
pub(super) struct TranscriptRows {
    pub(super) rows: Vec<AppTranscriptMessage>,
    latest_tools: HashMap<String, usize>,
}

#[cfg(test)]
mod tests;

impl TranscriptRows {
    pub(super) fn push(&mut self, row: AppTranscriptMessage) {
        if let Some(tool) = &row.tool {
            self.latest_tools
                .insert(tool.call_id.clone(), self.rows.len());
        }
        self.rows.push(row);
    }

    pub(super) fn clear(&mut self) {
        self.rows.clear();
        self.latest_tools.clear();
    }

    pub(super) fn tool_mut(&mut self, call_id: &str) -> Option<&mut AppTranscriptTool> {
        let index = *self.latest_tools.get(call_id)?;
        self.rows[index].tool.as_mut()
    }
}
