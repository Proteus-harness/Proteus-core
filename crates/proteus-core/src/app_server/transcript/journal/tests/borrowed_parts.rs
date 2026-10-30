use super::*;
use crate::model_standard::{CanonicalPart, MessagePhase};

fn image(id: &str) -> crate::domain::ImageRef {
    crate::domain::ImageRef {
        id: id.into(),
        name: format!("{id}.png"),
        mime_type: "image/png".into(),
        path: format!("/session/images/{id}").into(),
    }
}

#[test]
fn filtered_parts_preserve_text_groups_tool_updates_and_image_attachment() {
    let call = ToolCall::new("call-1", "probe", json!({"query": "value"}));
    let images = vec![image("first"), image("last")];
    let mut message = CanonicalMessage::new(
        MessageRole::Assistant,
        vec![
            ContentPart::Text {
                text: "before".into(),
            },
            ContentPart::Image {
                image: images[0].clone(),
            },
            ContentPart::Text {
                text: "same group".into(),
            },
            ContentPart::ToolCall { call: call.clone() },
            ContentPart::Text {
                text: "between".into(),
            },
            ContentPart::ToolResult {
                result: ToolResult::ok(call.id.clone(), "finished")
                    .with_metadata(json!({"source": "canonical"})),
            },
            ContentPart::Text {
                text: "after".into(),
            },
            ContentPart::Image {
                image: images[1].clone(),
            },
        ],
    );
    message.phase = Some(MessagePhase::FinalAnswer);
    message.parts.insert(
        1,
        CanonicalPart::new(
            PartProvenance::ContextBuilder,
            PartScope::Request,
            ContentPart::Image {
                image: image("hidden"),
            },
        ),
    );
    let original = message.clone();
    let mut state = TranscriptProjectionState::default();
    state.append_message(&message, false);

    assert_eq!(
        message, original,
        "projection must not mutate canonical data"
    );
    assert_eq!(state.transcript.len(), 4);
    assert_eq!(state.transcript[0].text, "before\nsame group");
    assert_eq!(state.transcript[2].text, "between");
    assert_eq!(state.transcript[3].text, "after");
    assert!(
        state.transcript[..3]
            .iter()
            .all(|row| row.images.is_empty())
    );
    assert_eq!(state.transcript[3].images, images);
    for index in [0, 2, 3] {
        assert_eq!(state.transcript[index].message_id, Some(message.id));
        assert_eq!(state.transcript[index].phase, message.phase);
        assert_eq!(state.transcript[index].role, "assistant");
    }
    let tool = state.transcript[1].tool.as_ref().unwrap();
    assert_eq!(tool.call_id, call.id);
    assert_eq!(tool.status, "done");
    assert_eq!(tool.result.as_deref(), Some("finished"));
    assert_eq!(tool.metadata["source"], "canonical");

    let previous = serde_json::to_value(&state.transcript).unwrap();
    state.append_message(&message, false);
    assert_eq!(serde_json::to_value(&state.transcript).unwrap(), previous);
    assert_eq!(state.seen_parts.len(), message.parts.len());
    assert_eq!(state.seen_calls.len(), 1);
    assert_eq!(state.seen_results.len(), 1);
}

#[test]
fn all_filtered_parts_still_consume_identities_until_reset() {
    let call = ToolCall::new("hidden-call", "probe", json!({}));
    let mut hidden = CanonicalMessage::new(
        MessageRole::Assistant,
        vec![
            ContentPart::Text {
                text: "summary".into(),
            },
            ContentPart::ToolCall { call: call.clone() },
            ContentPart::Image {
                image: image("request"),
            },
        ],
    );
    hidden.parts[0].provenance = PartProvenance::Compactor;
    hidden.parts[1].scope = PartScope::Trace;
    hidden.parts[2].scope = PartScope::Request;
    let mut state = TranscriptProjectionState::default();
    state.append_message(&hidden, true);
    assert!(state.transcript.is_empty());
    assert_eq!(state.seen_parts.len(), hidden.parts.len());
    assert!(state.seen_calls.is_empty());

    // A later non-compaction visit must not reveal an already-seen part.
    state.append_message(&hidden, false);
    assert!(state.transcript.is_empty());
    state.reset();
    state.append_message(&hidden, false);
    assert_eq!(state.transcript.len(), 1);
    assert_eq!(state.transcript[0].text, "summary");

    // Filtering a trace-only call did not consume the call ID itself.
    let visible =
        CanonicalMessage::new(MessageRole::Assistant, vec![ContentPart::ToolCall { call }]);
    state.append_message(&visible, false);
    assert_eq!(state.transcript.len(), 2);
    assert_eq!(
        state.transcript[1].tool.as_ref().unwrap().call_id,
        "hidden-call"
    );
}

#[test]
fn filtered_citations_update_prior_hosted_card_without_moving_images() {
    use crate::domain::{Citation, HostedToolActivity, HostedToolStatus, WebSearchAction};

    let first = CanonicalMessage::new(
        MessageRole::Assistant,
        vec![
            ContentPart::HostedToolActivity {
                activity: HostedToolActivity::WebSearch {
                    id: "hosted".into(),
                    status: HostedToolStatus::Completed,
                    action: WebSearchAction::Search {
                        queries: vec!["docs".into()],
                        sources: vec![],
                    },
                },
            },
            ContentPart::Image {
                image: image("first"),
            },
        ],
    );
    let citation = Citation::Url {
        start_index: 0,
        end_index: 1,
        title: "docs".into(),
        url: "https://example.com/docs".into(),
    };
    let mut second = CanonicalMessage::new(
        MessageRole::Assistant,
        vec![
            ContentPart::Citation {
                citation: citation.clone(),
            },
            ContentPart::Citation { citation },
            ContentPart::Image {
                image: image("second"),
            },
        ],
    );
    second.parts[1].scope = PartScope::Trace;
    let mut state = TranscriptProjectionState::default();
    state.append_message(&first, false);
    state.append_message(&second, false);
    state.append_message(&second, false);

    assert_eq!(state.transcript.len(), 3);
    let hosted = state.transcript[0].tool.as_ref().unwrap();
    assert_eq!(hosted.metadata["citations"].as_array().unwrap().len(), 1);
    assert_eq!(hosted.metadata["citations"][0]["type"], "url");
    assert!(state.transcript[0].images.is_empty());
    assert_eq!(state.transcript[1].message_id, Some(first.id));
    assert_eq!(state.transcript[1].images, vec![image("first")]);
    assert_eq!(state.transcript[2].message_id, Some(second.id));
    assert_eq!(state.transcript[2].images, vec![image("second")]);
}

#[test]
fn filtered_out_old_part_still_dates_a_rehosted_message() {
    let session_id = new_session_id();
    let thread_id = new_thread_id();
    let turn_id = new_turn_id();
    let old = CanonicalMessage::text(MessageRole::User, "retained");
    let mut rehosted = CanonicalMessage::text(MessageRole::Assistant, "new text");
    rehosted.parts.insert(0, old.parts[0].clone());
    let mut records = Vec::new();
    for (index, message) in [old.clone(), rehosted.clone()].into_iter().enumerate() {
        let sequence = index as u64 + 1;
        let mut item = record(
            session_id,
            None,
            thread_id,
            turn_id,
            sequence,
            JournalEntry::HistoryMutated(HistoryMutated {
                previous_revision: sequence - 1,
                new_revision: sequence,
                mutation: if index == 0 {
                    HistoryMutationKind::Append
                } else {
                    HistoryMutationKind::Replace
                },
                messages: vec![message],
                compaction: None,
                tool_results: vec![],
            }),
        );
        item.turn_id = None;
        item.timestamp_ms = if index == 0 { 100 } else { 50 };
        records.push(item);
    }
    let projection = JournalProjection::build(session_id, records).unwrap();
    let mut state = TranscriptProjectionState {
        timestamps: TranscriptTimestamps::from_projection(&projection),
        ..Default::default()
    };
    state.append_message(&old, false);
    state.append_message(&rehosted, false);
    assert_eq!(state.transcript.len(), 2);
    assert_eq!(state.transcript[1].text, "new text");
    assert_eq!(state.transcript[1].timestamp_ms, Some(100));
    assert_eq!(state.transcript[1].message_id, Some(rehosted.id));
}
