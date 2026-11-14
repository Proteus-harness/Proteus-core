//! Separate answer and readable reasoning rows retain the same canonical item id.
use super::{AppTranscriptMessage, TurnProgress};

impl TurnProgress {
    pub(super) fn update_text(
        &mut self,
        message_id: crate::domain::MessageId,
        phase: Option<crate::model_standard::MessagePhase>,
        text: &str,
        completed: bool,
        timestamp_ms: i64,
    ) {
        if let Some(message) = self
            .messages
            .iter_mut()
            .find(|message| message.message_id == Some(message_id) && message.role == "assistant")
        {
            if completed {
                message.text = text.to_owned();
            } else {
                message.text.push_str(text);
            }
            message.phase = if completed {
                phase
            } else {
                phase.or(message.phase)
            };
            message.streaming = !completed;
            return;
        }
        if text.is_empty() {
            return;
        }
        for message in &mut self.messages {
            message.streaming = false;
        }
        self.messages.push(AppTranscriptMessage {
            timestamp_ms: Some(timestamp_ms),
            images: Vec::new(),
            message_id: Some(message_id),
            phase,
            role: "assistant".to_owned(),
            text: text.to_owned(),
            tool: None,
            subagent: None,
            streaming: !completed,
        });
    }

    pub(super) fn update_reasoning(
        &mut self,
        message_id: crate::domain::MessageId,
        offset: Option<usize>,
        text: &str,
        timestamp_ms: i64,
    ) {
        if let Some(index) = self.messages.iter().position(|message| {
            message.message_id == Some(message_id) && message.role == "reasoning"
        }) {
            if offset.is_none() && text.trim().is_empty() {
                self.messages.remove(index);
                return;
            }
            let message = &mut self.messages[index];
            if let Some(offset) = offset {
                if offset > message.text.len() || !message.text.is_char_boundary(offset) {
                    return;
                }
                let overlap = (message.text.len() - offset).min(text.len());
                if !text.is_char_boundary(overlap)
                    || !message.text.is_char_boundary(offset + overlap)
                    || message.text[offset..offset + overlap] != text[..overlap]
                {
                    return;
                }
                message.text.push_str(&text[overlap..]);
            } else {
                message.text = text.to_owned();
            }
            message.streaming = offset.is_some();
            return;
        }
        if text.is_empty()
            || (offset.is_none() && text.trim().is_empty())
            || offset.is_some_and(|offset| offset != 0)
        {
            return;
        }
        for message in &mut self.messages {
            message.streaming = false;
        }
        let index = self
            .messages
            .iter()
            .position(|message| {
                message.message_id == Some(message_id) && message.role == "assistant"
            })
            .unwrap_or(self.messages.len());
        self.messages.insert(
            index,
            AppTranscriptMessage {
                timestamp_ms: Some(timestamp_ms),
                images: Vec::new(),
                message_id: Some(message_id),
                phase: None,
                role: "reasoning".to_owned(),
                text: text.to_owned(),
                tool: None,
                subagent: None,
                streaming: offset.is_some(),
            },
        );
    }
}
