use std::collections::HashMap;

use super::{AppTranscriptMessage, AppTranscriptTool};

/// One render's display rows and identity indexes. A late reasoning part can
/// insert before its existing answer; every index follows that insertion.
/// Neither result/resolution nor citation/image updates change call IDs or row
/// positions. A display reset must clear both rows and their lookup together.
#[derive(Default)]
pub(super) struct TranscriptRows {
    pub(super) rows: Vec<AppTranscriptMessage>,
    latest_tools: HashMap<String, usize>,
    reasoning_rows: HashMap<crate::domain::MessageId, usize>,
    answer_rows: HashMap<crate::domain::MessageId, usize>,
    // Only nonzero when the whole staged append is certified text-only.
    text_append_ceiling: usize,
}

#[cfg(test)]
mod tests;

impl TranscriptRows {
    /// A single text-only append produces at most one row per input message.
    /// Do not preallocate while all decoded inputs are still owned.
    pub(super) fn certify_text_append(&mut self, upper: usize) {
        // Keep the ordinary writer and sizing for short and oversized histories.
        if (4_096..=65_536).contains(&upper) {
            debug_assert!(self.rows.is_empty());
            self.text_append_ceiling = upper;
        }
    }

    fn reserve_text_append_tail(&mut self) {
        let cap = self.rows.capacity();
        if self.rows.len() == cap
            && self.text_append_ceiling > cap
            && self.text_append_ceiling < cap.saturating_mul(2)
        {
            // The row already passed filtering; growth is inevitable now.
            // A refused optional reserve leaves Vec's ordinary growth intact.
            let _ = self.rows.try_reserve_exact(self.text_append_ceiling - cap);
        }
    }

    pub(super) fn push(&mut self, row: AppTranscriptMessage) {
        if self.text_append_ceiling != 0 {
            self.reserve_text_append_tail();
            // No reasoning row can search old answer positions in this append.
        } else if row.role == "assistant"
            && let Some(id) = row.message_id
        {
            self.answer_rows.entry(id).or_insert(self.rows.len());
        }
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
        self.answer_rows.clear();
        self.text_append_ceiling = 0;
    }

    /// Journal part de-duplication happens before this method. Multiple readable
    /// parts of one canonical item share one separate display row.
    pub(super) fn append_reasoning(&mut self, message_id: crate::domain::MessageId, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if let Some(index) = self.reasoning_rows.get(&message_id) {
            let row = &mut self.rows[*index];
            let capacity = row.text.capacity();
            if capacity >= 8
                && row.text.len() == capacity
                && text.len() >= capacity.saturating_mul(3)
            {
                // Both pushes would grow this row. The final size dominates
                // both growth steps; keep that capacity and request it once.
                let _ = row.text.try_reserve(text.len() + 1);
            }
            row.text.push('\n');
            row.text.push_str(text);
            return;
        }
        let index = self
            .answer_rows
            .get(&message_id)
            .copied()
            .unwrap_or(self.rows.len());
        let timestamp_ms = self.rows.get(index).and_then(|row| row.timestamp_ms);
        let row = AppTranscriptMessage {
            message_id: Some(message_id),
            timestamp_ms,
            phase: None,
            role: "reasoning".to_owned(),
            text: text.to_owned(),
            images: Vec::new(),
            tool: None,
            subagent: None,
            streaming: false,
        };
        if index == self.rows.len() {
            self.push(row);
        } else {
            for position in self
                .latest_tools
                .values_mut()
                .chain(self.reasoning_rows.values_mut())
                .chain(self.answer_rows.values_mut())
            {
                if *position >= index {
                    *position += 1;
                }
            }
            self.reasoning_rows.insert(message_id, index);
            self.rows.insert(index, row);
        }
    }

    pub(super) fn tool_mut(&mut self, call_id: &str) -> Option<&mut AppTranscriptTool> {
        let index = *self.latest_tools.get(call_id)?;
        self.rows[index].tool.as_mut()
    }
}
