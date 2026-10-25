use super::*;
use crate::{
    contracts::ModelContextObservation,
    domain::{ImageRef, ToolCall, ToolContent, ToolResult, new_call_id, new_session_id},
    model_standard::{CanonicalMessage, ContentPart, MessagePhase, MessageRole},
};
use serde_json::json;

#[test]
fn conversation_view_roundtrip_retains_ordered_canonical_data_without_flattening() {
    let call = ToolCall::new(new_call_id(), "inspect", json!({"nested": [1, null]}))
        .with_raw_arguments("{ \"nested\": [1, null] }");
    let image = ImageRef {
        id: "image-sha256".to_owned(),
        name: "diagram.png".to_owned(),
        mime_type: "image/png".to_owned(),
        path: "/workspace/attachments/diagram.png".into(),
    };
    let user = CanonicalMessage::new(
        MessageRole::User,
        vec![
            ContentPart::Text {
                text: "first".into(),
            },
            ContentPart::Image { image },
            ContentPart::FileRef {
                path: "src/main.rs".into(),
                content: Some("fn main() {}".into()),
            },
            ContentPart::Text {
                text: "last".into(),
            },
        ],
    )
    .with_metadata(json!({"foreign": {"keep": [false, 42]}}));
    let assistant = CanonicalMessage::new(
        MessageRole::Assistant,
        vec![
            ContentPart::Reasoning {
                text: "reasoning".into(),
                signature: Some("opaque-signature".into()),
            },
            ContentPart::ToolCall { call: call.clone() },
        ],
    )
    .with_phase(MessagePhase::Commentary)
    .with_name("worker");
    let result = ToolResult::ok(call.id.clone(), "plain output")
        .with_content(vec![
            ToolContent::Json {
                value: json!({"structured": true}),
            },
            ToolContent::Image {
                mime_type: "image/png".into(),
                data: "opaque-image".into(),
            },
            ToolContent::Binary {
                mime_type: "application/octet-stream".into(),
                data: "opaque-bytes".into(),
            },
        ])
        .with_metadata(json!({"unknown-extension": {"preserve": "exactly"}}));
    let tool = CanonicalMessage::new(MessageRole::Tool, vec![ContentPart::ToolResult { result }])
        .with_tool_call_id(call.id);
    let snapshot = ConversationSnapshot {
        revision: 17,
        messages: vec![user, assistant, tool],
        model_context: vec![
            ModelContextObservation::Usage {
                total_tokens: 900,
                last_tokens: 300,
            },
            ModelContextObservation::HistoryCompacted,
            ModelContextObservation::ContextWindowExceeded {
                max_input_tokens: 128_000,
            },
        ],
    };
    let session_id = new_session_id();
    let view = ConversationView::new(session_id, "test-producer", snapshot.clone());
    let wire = serde_json::to_value(&view).unwrap();
    assert_eq!(wire["schema_version"], 1);
    assert_eq!(wire["projection"], "conversation");
    assert_eq!(wire["source"]["harness"], "proteus");
    assert_eq!(wire["source"]["version"], "test-producer");
    assert_eq!(wire["source"]["revision"], 17);
    let decoded: ConversationView = serde_json::from_value(wire).unwrap();
    assert_eq!(decoded.source.session_id, session_id);
    // Equality includes message/part IDs, order, provenance, scope, phase,
    // references, raw args, structured tool content and opaque metadata.
    assert_eq!(decoded.conversation, snapshot);
}
