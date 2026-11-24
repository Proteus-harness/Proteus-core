use serde_json::Value;

use crate::{
    domain::{Citation, HostedToolActivity, HostedToolStatus, ToolCall, ToolResult},
    model_standard::{CanonicalMessage, CanonicalPart, ContentPart, MessageRole},
};

mod journal;
mod rows;
use rows::TranscriptRows;

#[cfg(test)]
pub(crate) use journal::journal_transcript_messages;
pub(crate) use journal::{
    journal_transcript_append_committed, journal_transcript_append_from_source,
    journal_transcript_from_source,
};

pub use proteus_contracts::app_protocol::{
    AppTranscriptMessage, AppTranscriptSubagent, AppTranscriptTool,
};

pub(super) fn transcript_messages(messages: &[CanonicalMessage]) -> Vec<AppTranscriptMessage> {
    let mut transcript = TranscriptRows::default();
    for message in messages {
        append_transcript_message(&mut transcript, message);
    }
    finalize_interrupted_tools(&mut transcript.rows);
    transcript.rows
}

fn append_transcript_message(transcript: &mut TranscriptRows, message: &CanonicalMessage) {
    append_transcript_parts(transcript, message, &message.parts);
}

fn append_transcript_parts<'a>(
    transcript: &mut TranscriptRows,
    message: &CanonicalMessage,
    parts: impl IntoIterator<Item = &'a CanonicalPart>,
) {
    let role = transcript_role(&message.role);
    let mut text_parts = PendingText::default();
    let mut images = Vec::new();
    for part in parts {
        match &part.payload {
            ContentPart::Reasoning { text, .. } | ContentPart::ReasoningSummary { text }
                if message.role == MessageRole::Assistant =>
            {
                transcript.append_reasoning(message.id, text);
            }
            ContentPart::Image { image } => images.push(image.clone()),
            ContentPart::Text { text } if !text.trim().is_empty() => {
                text_parts.push(text.as_str());
            }
            ContentPart::ToolCall { call } => {
                flush_transcript_text(transcript, role, message, &mut text_parts);
                append_transcript_tool_call(transcript, call);
            }
            ContentPart::ToolResult { result } => {
                flush_transcript_text(transcript, role, message, &mut text_parts);
                append_transcript_tool_result(transcript, result);
            }
            ContentPart::HostedToolActivity { activity } => {
                flush_transcript_text(transcript, role, message, &mut text_parts);
                append_hosted_tool_activity(transcript, activity);
            }
            ContentPart::Citation { citation } => {
                append_citation_metadata(&mut transcript.rows, citation);
            }
            _ => {}
        }
    }
    flush_transcript_text(transcript, role, message, &mut text_parts);
    if !images.is_empty() {
        if let Some(item) = transcript
            .rows
            .last_mut()
            .filter(|item| item.message_id == Some(message.id) && item.role == role)
        {
            item.images = images;
        } else {
            transcript.push(AppTranscriptMessage {
                timestamp_ms: None,
                message_id: Some(message.id),
                phase: message.phase,
                role: role.to_owned(),
                text: String::new(),
                images,
                tool: None,
                subagent: None,
                streaming: false,
            });
        }
    }
}

fn append_transcript_tool_call(transcript: &mut TranscriptRows, call: &ToolCall) {
    transcript.push(AppTranscriptMessage {
        timestamp_ms: None,
        images: Vec::new(),
        message_id: None,
        phase: None,
        role: "system".to_owned(),
        text: String::new(),
        tool: Some(AppTranscriptTool {
            terminal: None,
            effective_args: None,
            call_id: call.id.clone(),
            name: call.name.clone(),
            args: call.args.clone(),
            status: "running".to_owned(),
            result: None,
            metadata: Value::Null,
        }),
        subagent: None,
        streaming: false,
    });
}

fn finalize_interrupted_tools(transcript: &mut [AppTranscriptMessage]) {
    // Durable history/journal no longer has a live producer. A call without a
    // terminal result is interrupted, not an eternally running UI activity.
    for message in transcript {
        if let Some(tool) = message
            .tool
            .as_mut()
            .filter(|tool| tool.status == "running")
        {
            tool.status = "interrupted".to_owned();
        }
    }
}

fn append_hosted_tool_activity(transcript: &mut TranscriptRows, activity: &HostedToolActivity) {
    let status = match activity.status() {
        HostedToolStatus::Completed => "done",
        HostedToolStatus::Failed => "failed",
        HostedToolStatus::InProgress | HostedToolStatus::Searching => "running",
        HostedToolStatus::Unknown(_) => activity.status().as_str(),
        _ => "unknown",
    };
    let args = match activity {
        HostedToolActivity::WebSearch { action, .. } => {
            serde_json::to_value(action).unwrap_or(Value::Null)
        }
        HostedToolActivity::FileSearch {
            queries, results, ..
        } => serde_json::json!({
            "queries": queries,
            "result_count": results.len(),
        }),
        _ => Value::Null,
    };
    let result = match activity {
        HostedToolActivity::WebSearch { action, .. } => match action {
            crate::domain::WebSearchAction::Search { queries, sources } => Some(format!(
                "{} search quer{}; {} source{}",
                queries.len(),
                if queries.len() == 1 { "y" } else { "ies" },
                sources.len(),
                if sources.len() == 1 { "" } else { "s" }
            )),
            crate::domain::WebSearchAction::OpenPage { url } => Some(format!("opened {url}")),
            crate::domain::WebSearchAction::FindInPage { url, pattern } => {
                Some(format!("searched {url} for {pattern}"))
            }
            crate::domain::WebSearchAction::Unknown { name, .. } => {
                Some(format!("provider action: {name}"))
            }
            _ => None,
        },
        HostedToolActivity::FileSearch {
            queries, results, ..
        } => Some(format!(
            "{} quer{}; {} result{}",
            queries.len(),
            if queries.len() == 1 { "y" } else { "ies" },
            results.len(),
            if results.len() == 1 { "" } else { "s" }
        )),
        _ => None,
    };
    transcript.push(AppTranscriptMessage {
        timestamp_ms: None,
        images: Vec::new(),
        message_id: None,
        phase: None,
        role: "system".to_owned(),
        text: String::new(),
        tool: Some(AppTranscriptTool {
            terminal: None,
            effective_args: None,
            call_id: activity.id().to_owned(),
            name: activity.kind().as_str().to_owned(),
            args,
            status: status.to_owned(),
            result,
            metadata: serde_json::json!({
                "provider_hosted": true,
                "activity": activity,
                "citations": [],
            }),
        }),
        subagent: None,
        streaming: false,
    });
}

fn append_citation_metadata(transcript: &mut [AppTranscriptMessage], citation: &Citation) {
    let tool_name = match citation {
        Citation::Url { .. } => "web_search",
        Citation::File { .. } => "file_search",
        _ => return,
    };
    let Some(tool) = transcript.iter_mut().rev().find_map(|message| {
        message.tool.as_mut().filter(|tool| {
            tool.name == tool_name
                && tool
                    .metadata
                    .get("provider_hosted")
                    .and_then(Value::as_bool)
                    == Some(true)
        })
    }) else {
        return;
    };
    let Some(citations) = tool
        .metadata
        .get_mut("citations")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    if let Ok(citation) = serde_json::to_value(citation) {
        citations.push(citation);
    }
}

/// The common single-part segment needs no heap allocation before the final
/// row string. Keep the same joined allocation and reuse scratch for longer
/// segments within the same message.
#[derive(Default)]
struct PendingText<'a> {
    last: Option<&'a str>,
    earlier: Vec<&'a str>,
}

impl<'a> PendingText<'a> {
    fn push(&mut self, text: &'a str) {
        if let Some(previous) = self.last.replace(text) {
            self.earlier.push(previous);
        }
    }

    fn take_joined(&mut self) -> Option<String> {
        let last = self.last.take()?;
        if self.earlier.is_empty() {
            return Some(last.to_owned());
        }
        self.earlier.push(last);
        let text = self.earlier.join("\n");
        self.earlier.clear();
        Some(text)
    }
}

fn flush_transcript_text(
    transcript: &mut TranscriptRows,
    role: &str,
    message: &CanonicalMessage,
    text_parts: &mut PendingText<'_>,
) {
    let Some(text) = text_parts.take_joined() else {
        return;
    };
    push_transcript_text(transcript, role, message, text, None);
}

fn push_transcript_text(
    transcript: &mut TranscriptRows,
    role: &str,
    message: &CanonicalMessage,
    text: String,
    timestamp_ms: Option<i64>,
) {
    transcript.push(AppTranscriptMessage {
        timestamp_ms,
        images: Vec::new(),
        message_id: Some(message.id),
        phase: message.phase,
        role: role.to_owned(),
        text,
        tool: None,
        subagent: None,
        streaming: false,
    });
}

fn append_transcript_tool_result(transcript: &mut TranscriptRows, result: &ToolResult) {
    let status = if result.ok { "done" } else { "failed" }.to_owned();
    let result_text = result.text_or_status();
    apply_transcript_tool_result(transcript, result, status, result_text);
}

fn append_owned_transcript_tool_result(transcript: &mut TranscriptRows, result: &mut ToolResult) {
    let status = if result.ok { "done" } else { "failed" }.to_owned();
    let selected = match &mut result.error {
        Some(error) if !error.is_empty() => Some(error),
        _ if !result.output.is_empty() => Some(&mut result.output),
        _ => None,
    };
    // Match the borrowed result's capacity as well as its contents.
    let result_text = match selected {
        Some(text) if text.len() == text.capacity() => std::mem::take(text),
        _ => result.text_or_status(),
    };
    apply_transcript_tool_result(transcript, result, status, result_text);
}

fn apply_transcript_tool_result(
    transcript: &mut TranscriptRows,
    result: &ToolResult,
    status: String,
    result_text: String,
) {
    let terminal = result
        .metadata
        .get("tool_terminal")
        .and_then(|value| serde::Deserialize::deserialize(value).ok());
    if let Some(tool) = transcript.tool_mut(&result.call_id) {
        tool.terminal = terminal;
        tool.status = status;
        tool.result = Some(result_text);
        tool.metadata = result.metadata.clone();
        return;
    }

    transcript.push(AppTranscriptMessage {
        timestamp_ms: None,
        images: Vec::new(),
        message_id: None,
        phase: None,
        role: "system".to_owned(),
        text: String::new(),
        tool: Some(AppTranscriptTool {
            terminal,
            effective_args: None,
            call_id: result.call_id.clone(),
            name: "tool".to_owned(),
            args: Value::Null,
            status,
            result: Some(result_text),
            metadata: result.metadata.clone(),
        }),
        subagent: None,
        streaming: false,
    });
}

fn transcript_role(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System | MessageRole::Developer => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "system",
        _ => "system",
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn image_only_user_message_has_a_durable_transcript_item() {
        use super::*;
        let image = crate::domain::ImageRef {
            id: "image-id".into(),
            name: "board.png".into(),
            mime_type: "image/png".into(),
            path: "/session/images/image-id".into(),
        };
        let message = CanonicalMessage::new(
            MessageRole::User,
            vec![ContentPart::Image {
                image: image.clone(),
            }],
        );
        let transcript = transcript_messages(&[message.clone()]);
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].message_id, Some(message.id));
        assert_eq!(transcript[0].images, vec![image]);
        assert!(transcript[0].text.is_empty());
    }
    use serde_json::json;

    use super::*;
    use crate::domain::{FileSearchResult, ToolCall, WebSearchAction, WebSearchSource};

    #[test]
    fn reasoning_is_not_ordinary_assistant_text() {
        let reasoning = CanonicalMessage::new(
            MessageRole::Assistant,
            vec![
                ContentPart::ReasoningSummary {
                    text: "summary".to_owned(),
                },
                ContentPart::Reasoning {
                    text: "reasoning".to_owned(),
                    signature: None,
                },
            ],
        );
        let mut mixed = reasoning.clone();
        mixed.phase = Some(crate::model_standard::MessagePhase::FinalAnswer);
        mixed
            .parts
            .extend(CanonicalMessage::text(MessageRole::Assistant, "answer").parts);

        let reasoning_rows = transcript_messages(&[reasoning]);
        assert_eq!(reasoning_rows.len(), 1);
        assert_eq!(reasoning_rows[0].role, "reasoning");
        assert_eq!(reasoning_rows[0].text, "summary\nreasoning");
        mixed.parts.push(crate::model_standard::CanonicalPart::new(
            crate::model_standard::PartProvenance::Model,
            crate::model_standard::PartScope::Conversation,
            ContentPart::Opaque {
                format: "provider-hidden".into(),
                data: json!({"secret":"opaque"}),
            },
        ));
        let transcript = transcript_messages(&[mixed.clone()]);
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].role, "reasoning");
        assert_eq!(transcript[0].text, "summary\nreasoning");
        assert_eq!(transcript[1].text, "answer");
        assert_eq!(transcript[0].message_id, Some(mixed.id));
        assert_eq!(transcript[1].message_id, Some(mixed.id));
        assert_eq!(transcript[1].phase, mixed.phase);
        assert_eq!(mixed.display_text(), "answer");
    }

    #[test]
    fn visible_text_keeps_segments_between_tool_cards() {
        let text = |s: &str| ContentPart::Text { text: s.to_owned() };
        let call = |id: &str| ContentPart::ToolCall {
            call: ToolCall::new(id, "read", json!({})),
        };
        let message = CanonicalMessage::new(
            MessageRole::Assistant,
            vec![
                text(" first "),
                call("1"),
                text("\u{2002}"),
                text("two"),
                text("三"),
                call("2"),
                text("🌊 trailing "),
            ],
        );
        let rows = transcript_messages(&[message]);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].text, " first ");
        assert_eq!(rows[1].tool.as_ref().unwrap().call_id, "1");
        assert_eq!(rows[2].text, "two\n三");
        assert_eq!(rows[3].tool.as_ref().unwrap().call_id, "2");
        assert_eq!(rows[4].text, "🌊 trailing ");
    }

    #[test]
    fn committed_tool_call_without_result_becomes_interrupted() {
        // Закоммиченная история пишется в конце хода: вызов без результата —
        // прерванный, а не бегущий; "running" дал бы вечный спиннер в UI.
        let call = CanonicalMessage::new(
            MessageRole::Assistant,
            vec![ContentPart::ToolCall {
                call: ToolCall::new("call-1", "shell", json!({ "command": "ls" })),
            }],
        );

        let transcript = transcript_messages(&[call]);

        assert_eq!(transcript.len(), 1);
        let tool = transcript[0].tool.as_ref().expect("tool card");
        assert_eq!(tool.status, "interrupted");
    }

    #[test]
    fn tool_result_metadata_passes_through_to_transcript_card() {
        let call = CanonicalMessage::new(
            MessageRole::Assistant,
            vec![ContentPart::ToolCall {
                call: ToolCall::new("call-1", "task", json!({ "agent_type": "explore" })),
            }],
        );
        let result = CanonicalMessage::new(
            MessageRole::Tool,
            vec![ContentPart::ToolResult {
                result: ToolResult::ok("call-1".to_owned(), "summary").with_metadata(json!({
                    "status": "completed",
                    "child_thread_id": "child-thread",
                })),
            }],
        );

        let transcript = transcript_messages(&[call, result]);

        assert_eq!(transcript.len(), 1);
        let tool = transcript[0].tool.as_ref().expect("tool card");
        assert_eq!(tool.status, "done");
        // Metadata результата не теряется на границе /history: клиент строит
        // по ней карточку субагента, core имён tools не знает.
        assert_eq!(tool.metadata["child_thread_id"], "child-thread");
        assert_eq!(tool.metadata["status"], "completed");
    }

    #[test]
    fn terminal_metadata_is_projected_without_changing_the_raw_result() {
        let call = ToolCall::new("terminal-call", "fixture", json!({}));
        let terminal = crate::contracts::ToolTerminalRef {
            id: uuid::Uuid::from_u128(42),
            title: "terminal 🌱".into(),
            tty: true,
        };
        let valid = json!(terminal);
        let mut unknown_field = valid.clone();
        unknown_field["extra"] = json!({"ignored": [1,2,3]});
        let mut bad_id = valid.clone();
        bad_id["id"] = json!("not a UUID");
        let mut missing_tty = valid.clone();
        missing_tty.as_object_mut().unwrap().remove("tty");
        let mut bad_title = valid.clone();
        bad_title["title"] = json!(["not a string"]);

        let cases = [
            Some(valid),
            Some(json!([terminal.id, terminal.title, terminal.tty])),
            Some(unknown_field),
            Some(bad_id),
            Some(missing_tty),
            Some(bad_title),
            Some(Value::Null),
            None,
        ];
        for preexisting in [false, true] {
            for value in &cases {
                let expected: Option<crate::contracts::ToolTerminalRef> = value
                    .as_ref()
                    .and_then(|value| serde_json::from_value(value.clone()).ok());
                let mut rows = TranscriptRows::default();
                if preexisting {
                    append_transcript_tool_call(&mut rows, &call);
                    // Invalid or absent metadata still clears an existing terminal.
                    rows.rows[0].tool.as_mut().unwrap().terminal = Some(terminal.clone());
                }
                let mut metadata = json!({"fixture": "keep"});
                if let Some(value) = value {
                    metadata["tool_terminal"] = value.clone();
                }
                let result =
                    ToolResult::ok(call.id.clone(), "synthetic").with_metadata(metadata.clone());
                append_transcript_tool_result(&mut rows, &result);
                assert_eq!(rows.rows.len(), 1);
                let tool = rows.rows[0].tool.as_ref().unwrap();
                assert_eq!(tool.terminal, expected);
                assert_eq!(tool.metadata, metadata);
                assert_eq!(tool.status, "done");
            }
        }
    }

    #[test]
    fn hosted_activity_and_citations_project_to_matching_tool_cards() {
        let message = CanonicalMessage::new(
            MessageRole::Assistant,
            vec![
                ContentPart::HostedToolActivity {
                    activity: HostedToolActivity::WebSearch {
                        id: "ws-1".to_owned(),
                        status: HostedToolStatus::Completed,
                        action: WebSearchAction::Search {
                            queries: vec!["current docs".to_owned()],
                            sources: vec![WebSearchSource::new(
                                "https://developers.openai.com",
                                Some("OpenAI docs".to_owned()),
                                Some("url".to_owned()),
                            )],
                        },
                    },
                },
                ContentPart::HostedToolActivity {
                    activity: HostedToolActivity::FileSearch {
                        id: "fs-1".to_owned(),
                        status: HostedToolStatus::Completed,
                        queries: vec!["architecture".to_owned()],
                        results: vec![FileSearchResult::new(
                            "file-1",
                            Some("architecture.pdf".to_owned()),
                            Some(0.9),
                            None,
                            Value::Null,
                        )],
                    },
                },
                ContentPart::Text {
                    text: "answer".to_owned(),
                },
                ContentPart::Citation {
                    citation: Citation::Url {
                        start_index: 0,
                        end_index: 3,
                        title: "OpenAI docs".to_owned(),
                        url: "https://developers.openai.com".to_owned(),
                    },
                },
                ContentPart::Citation {
                    citation: Citation::File {
                        index: 4,
                        file_id: "file-1".to_owned(),
                        filename: "architecture.pdf".to_owned(),
                    },
                },
            ],
        );

        let transcript = transcript_messages(&[message]);
        let web = transcript
            .iter()
            .filter_map(|message| message.tool.as_ref())
            .find(|tool| tool.name == "web_search")
            .expect("web search card");
        let file = transcript
            .iter()
            .filter_map(|message| message.tool.as_ref())
            .find(|tool| tool.name == "file_search")
            .expect("file search card");

        assert_eq!(web.status, "done");
        assert_eq!(web.metadata["citations"][0]["type"], "url");
        assert_eq!(file.status, "done");
        assert_eq!(file.metadata["citations"][0]["type"], "file");
        assert!(
            transcript
                .iter()
                .any(|message| message.role == "assistant" && message.text == "answer")
        );
    }
}
