use super::*;
use crate::domain::{HostedToolActivity, HostedToolStatus, WebSearchAction};

#[test]
fn resolution_targets_latest_hosted_collision_and_reset_forgets_it() {
    let call = ToolCall::new("same", "probe", json!({"effective": true}));
    let mut state = TranscriptProjectionState::default();
    state.append_tool_call(&call);
    state.append_message(
        &CanonicalMessage::new(
            MessageRole::Assistant,
            vec![ContentPart::HostedToolActivity {
                activity: HostedToolActivity::WebSearch {
                    id: call.id.clone(),
                    status: HostedToolStatus::Completed,
                    action: WebSearchAction::Search {
                        queries: vec![],
                        sources: vec![],
                    },
                },
            }],
        ),
        false,
    );
    state.apply_resolution(&call, &ToolCallResolution::Allowed);
    assert!(
        state.transcript.rows[0]
            .tool
            .as_ref()
            .unwrap()
            .effective_args
            .is_none()
    );
    assert_eq!(
        state.transcript.rows[1]
            .tool
            .as_ref()
            .unwrap()
            .effective_args,
        Some(call.args.clone())
    );
    state.apply_resolution(
        &call,
        &ToolCallResolution::PolicyDenied {
            reason: "denied".into(),
        },
    );
    assert_eq!(
        state.transcript.rows[0].tool.as_ref().unwrap().status,
        "running"
    );
    assert_eq!(
        state.transcript.rows[1]
            .tool
            .as_ref()
            .unwrap()
            .result
            .as_deref(),
        Some("denied")
    );

    state.reset();
    state.append_message(
        &CanonicalMessage::text(MessageRole::User, "after reset"),
        false,
    );
    state.apply_resolution(&call, &ToolCallResolution::Allowed);
    assert_eq!(state.transcript.rows.len(), 1);
    assert!(state.transcript.rows[0].tool.is_none());
    state.append_tool_result(&ToolResult::ok(call.id.clone(), "late"));
    assert_eq!(state.transcript.rows.len(), 2);
    assert_eq!(state.transcript.rows[1].tool.as_ref().unwrap().name, "tool");
}
