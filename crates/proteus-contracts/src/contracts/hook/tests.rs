use super::*;
use crate::domain::{ModelRef, ToolCall, ToolResult, new_call_id};
#[test]
fn tool_selection_chain_only_narrows_and_preserves_full_specs() {
    use super::super::{ToolSelectionInput, ToolSelectionOutput, ToolSelectionRequest};
    use crate::domain::{ToolSafety, ToolSurface};
    let first = ToolSpec::new("read", "Read", serde_json::json!({}), ToolSafety::ReadOnly);
    let second = ToolSpec::new(
        "write",
        "Write",
        serde_json::json!({}),
        ToolSafety::WritesFiles,
    );
    let third = ToolSpec::new(
        "shell",
        "Shell",
        serde_json::json!({}),
        ToolSafety::RunsCommands,
    );
    let input = ToolSelectionInput::new(
        ToolSelectionRequest::new(AgentTask::new("probe", PathBuf::from("."))).with_phase("review"),
        vec![first.clone(), second.clone(), third.clone()],
    );
    let event = HookEvent::BeforeToolSelection {
        input: input.clone(),
        output: ToolSelectionOutput::new(input.candidates.clone()),
    };
    let contribution = ToolSelectionOutput {
        tools: vec![third.clone(), first.clone()],
        metadata: serde_json::json!({"source": "first"}),
    };
    let narrowed = apply_hook_response(
        &event,
        &HookResponse::ToolSelection {
            output: contribution.clone(),
        },
    )
    .unwrap();
    assert_eq!(
        narrowed,
        HookEvent::BeforeToolSelection {
            input: input.clone(),
            output: contribution,
        }
    );
    assert_eq!(
        apply_hook_response(&narrowed, &HookResponse::Continue).unwrap(),
        narrowed
    );
    assert!(
        apply_hook_response(
            &narrowed,
            &HookResponse::ToolSelection {
                output: ToolSelectionOutput::new(vec![second]),
            }
        )
        .is_err(),
        "later hooks cannot restore omitted tools"
    );
    assert!(
        apply_hook_response(
            &narrowed,
            &HookResponse::ToolSelection {
                output: ToolSelectionOutput::new(vec![first.clone(), first.clone()]),
            }
        )
        .is_err(),
        "duplicate selections must be rejected"
    );
    let mut mutations = vec![];
    let mut changed = first.clone();
    changed.name = "invented".into();
    mutations.push(changed);
    let mut changed = first.clone();
    changed.description = "Changed".into();
    mutations.push(changed);
    let mut changed = first.clone();
    changed.input_schema = serde_json::json!({"type":"string"});
    mutations.push(changed);
    let mut changed = first.clone();
    changed.surface = ToolSurface::strict_function();
    mutations.push(changed);
    let mut changed = first.clone();
    changed.safety = ToolSafety::Dangerous;
    mutations.push(changed);
    let mut changed = first.clone();
    changed.supports_parallel_tool_calls = true;
    mutations.push(changed);
    let mut changed = first.clone();
    changed.timeout_ms = Some(1);
    mutations.push(changed);
    let mut changed = first;
    changed.metadata = serde_json::json!({"changed":true});
    mutations.push(changed);
    for changed in mutations {
        assert!(
            apply_hook_response(
                &narrowed,
                &HookResponse::ToolSelection {
                    output: ToolSelectionOutput::new(vec![changed]),
                }
            )
            .is_err(),
            "every ToolSpec field remains immutable"
        );
    }
    let empty = ToolSelectionOutput::new(vec![]);
    assert_eq!(
        apply_hook_response(
            &narrowed,
            &HookResponse::ToolSelection {
                output: empty.clone(),
            }
        )
        .unwrap(),
        HookEvent::BeforeToolSelection {
            input,
            output: empty.clone()
        }
    );
    let wrong_event = HookEvent::TurnStarted {
        task: AgentTask::new("probe", PathBuf::from(".")),
        history: vec![],
    };
    assert!(
        apply_hook_response(&wrong_event, &HookResponse::ToolSelection { output: empty }).is_err()
    );
    assert!(
        apply_hook_response(
            &narrowed,
            &HookResponse::ModelContext {
                messages: vec![],
                instructions: vec![],
            }
        )
        .is_err()
    );
}

#[test]
fn tool_selection_wire_is_strict_at_all_levels() {
    use super::super::{ToolSelectionInput, ToolSelectionOutput, ToolSelectionRequest};
    let event = HookEvent::BeforeToolSelection {
        input: ToolSelectionInput::new(
            ToolSelectionRequest::new(AgentTask::new("probe", PathBuf::from("."))),
            vec![],
        ),
        output: ToolSelectionOutput::new(vec![]),
    };
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(
        serde_json::from_value::<HookEvent>(value.clone()).unwrap(),
        event
    );
    for field in ["input", "output"] {
        let mut altered = value.clone();
        altered[field]["extra"] = serde_json::json!(true);
        assert!(serde_json::from_value::<HookEvent>(altered).is_err());
    }
    let mut altered = value;
    altered["input"]["request"]["extra"] = serde_json::json!(true);
    assert!(serde_json::from_value::<HookEvent>(altered).is_err());
    let response = HookResponse::ToolSelection {
        output: ToolSelectionOutput::new(vec![]),
    };
    let mut value = serde_json::to_value(&response).unwrap();
    assert_eq!(
        serde_json::from_value::<HookResponse>(value.clone()).unwrap(),
        response
    );
    value["output"]["extra"] = serde_json::json!(true);
    assert!(serde_json::from_value::<HookResponse>(value).is_err());
}

#[test]
fn responses_cannot_change_identity_or_status() {
    let id = new_call_id();
    let event = HookEvent::AfterTool {
        call: ToolCall::new(id.clone(), "read", serde_json::json!({})),
        result: ToolResult::error(id, "original"),
    };
    let transformed = apply_hook_response(
        &event,
        &HookResponse::ToolOutput {
            output: "redacted".into(),
        },
    )
    .unwrap();
    let HookEvent::AfterTool { call, mut result } = transformed else {
        panic!()
    };
    result.output = String::new();
    assert_eq!(HookEvent::AfterTool { call, result }, event);
    assert!(
        apply_hook_response(
            &event,
            &HookResponse::BlockTool {
                reason: "no".into()
            }
        )
        .is_err()
    );
    let model = HookEvent::BeforeModel {
        origin: ModelCallOrigin::Direct,
        request: CanonicalModelRequest::new(ModelRef::new("fake", "x"), vec![]),
    };
    assert!(apply_hook_response(&model, &HookResponse::ToolOutput { output: "x".into() }).is_err());
}
#[test]
fn notification_and_blank_blocks_are_rejected_strictly() {
    let notification = HookEvent::TurnSettled {
        status: HookTurnStatus::Success,
        output: None,
        error: None,
    };
    assert!(
        apply_hook_response(
            &notification,
            &HookResponse::ModelContext {
                messages: vec![],
                instructions: vec![]
            }
        )
        .is_err()
    );
    let before = HookEvent::BeforeTool {
        call: ToolCall::new(new_call_id(), "read", serde_json::json!({})),
        spec: None,
        blocked: None,
    };
    assert!(
        apply_hook_response(
            &before,
            &HookResponse::BlockTool {
                reason: "  ".into()
            }
        )
        .is_err()
    );
    assert!(serde_json::from_str::<HookResponse>(r#"{"action":"continue","extra":true}"#).is_err());
}
#[test]
fn model_context_cannot_rewrite_existing_canonical_part() {
    let original = CanonicalMessage::text(crate::model_standard::MessageRole::User, "original");
    let event = HookEvent::BeforeModel {
        origin: ModelCallOrigin::Direct,
        request: CanonicalModelRequest::new(ModelRef::new("fake", "x"), vec![original.clone()]),
    };
    let mut changed = original;
    changed.parts[0].payload = crate::model_standard::ContentPart::Text {
        text: "rewritten".into(),
    };
    assert!(
        apply_hook_response(
            &event,
            &HookResponse::ModelContext {
                messages: vec![changed],
                instructions: vec![]
            }
        )
        .unwrap_err()
        .to_string()
        .contains("immutable part")
    );
}
