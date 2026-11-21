use proteus_contracts::model_standard::{CanonicalMessage, ContentPart, MessageRole};

pub(super) fn session_preview(messages: &[CanonicalMessage]) -> Option<String> {
    messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .chain(messages.iter())
        .find_map(|message| {
            message.parts.iter().find_map(|part| match &part.payload {
                ContentPart::Text { text }
                | ContentPart::Reasoning { text, .. }
                | ContentPart::ReasoningSummary { text } => preview_text(text),
                ContentPart::ToolResult { result } => preview_text(&result.text_or_status()),
                _ => None,
            })
        })
}

fn preview_text(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // The public preview limit counts Unicode scalar values, not bytes.
    const LIMIT: usize = 160;
    let Some((end, _)) = text.char_indices().nth(LIMIT) else {
        return Some(text.to_owned());
    };
    let mut preview = String::with_capacity(end + 3);
    preview.push_str(&text[..end]);
    preview.push_str("...");
    Some(preview)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_trims_before_testing_the_unicode_limit() {
        let exact = "a界🙂е\u{301}".repeat(32);
        assert_eq!(
            preview_text(&format!("\u{2003}{exact}\n")),
            Some(exact.clone())
        );
        assert_eq!(
            preview_text(&format!(" {exact}Ж ")),
            Some(format!("{exact}..."))
        );
        let ends_in_space = format!("{} ", "Ж".repeat(159));
        assert_eq!(
            preview_text(&format!("{ends_in_space}字")),
            Some(format!("{ends_in_space}..."))
        );
        assert_eq!(preview_text("\t\u{a0}\n"), None);
    }

    #[test]
    fn preview_prefers_nonblank_user_parts_then_falls_back_in_history_order() {
        let assistant = CanonicalMessage::new(
            MessageRole::Assistant,
            vec![ContentPart::ReasoningSummary {
                text: " fallback ".into(),
            }],
        );
        let blank = CanonicalMessage::text(MessageRole::User, " \n ");
        let visible = CanonicalMessage::text(MessageRole::User, " preferred ");
        assert_eq!(
            session_preview(&[assistant.clone(), blank.clone()]).as_deref(),
            Some("fallback")
        );
        assert_eq!(
            session_preview(&[assistant, blank, visible]).as_deref(),
            Some("preferred")
        );
        assert_eq!(session_preview(&[]), None);
    }
}
