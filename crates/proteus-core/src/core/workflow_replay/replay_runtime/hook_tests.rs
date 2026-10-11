use super::super::super::fixture::RecordedToolInvocation;
use super::*;
use crate::{
    contracts::{ExecutionAttribution, HookStep, HookTrace, apply_hook_response},
    domain::{
        AgentTask, ReasoningConfig, ToolCall, ToolCallResolution, ToolResult, new_execution_id,
        new_message_id,
    },
};

fn input(event: HookEvent) -> HookInput {
    HookInput {
        conversation: None,
        event,
        attribution: ExecutionAttribution::detached(new_execution_id()),
        cwd: ".".into(),
    }
}
fn state(hooks: Vec<HookTrace>, tools: Vec<RecordedToolInvocation>) -> Arc<ReplayState> {
    Arc::new(ReplayState::new(
        vec![],
        hooks,
        tools,
        vec![],
        None,
        Default::default(),
        &ReasoningConfig::default(),
        new_message_id(),
    ))
}

#[tokio::test]
async fn recorded_hook_replay_preserves_strict_schema_after_sensitive_arguments_redaction() {
    let call = ToolCall::new(
        "redacted-call",
        "fixture",
        serde_json::json!({"password":"[REDACTED]"}),
    );
    let spec = crate::domain::ToolSpec::new(
        "fixture",
        "fixture",
        serde_json::json!({"type":"object", "properties":{"password":{"type":"number","const":12345}},"required":["password"]}),
        crate::domain::ToolSafety::ReadOnly,
    );
    let before = input(HookEvent::BeforeTool {
        call,
        spec: Some(spec.clone()),
        blocked: None,
    });
    let response = HookResponse::ToolArguments {
        args: serde_json::json!({"password":"[REDACTED]"}),
    };
    assert!(
        apply_hook_response(&before.event, &response).is_err(),
        "live schema remains strict"
    );
    let output = apply_recorded_hook_response(&before.event, &response).unwrap();
    let replay = ReplayHooks::new(
        state(
            vec![HookTrace {
                owner: crate::contracts::OperationOwner::Workflow,
                input: before.clone(),
                steps: vec![HookStep {
                    invocation_id: uuid::Uuid::new_v4(),
                    module_id: "rewrite".into(),
                    outcome: HookStepOutcome::Accepted { response },
                }],
                output: Some(output),
            }],
            vec![],
        ),
        vec!["rewrite".into()],
    );
    let HookEvent::BeforeTool {
        call,
        spec: Some(recorded_spec),
        ..
    } = replay.apply(before).await.unwrap()
    else {
        panic!("before tool")
    };
    assert_eq!(recorded_spec, spec);
    assert_eq!(call.args["password"], "[REDACTED]");
}

#[tokio::test]
async fn tool_replay_uses_raw_outcome_then_applies_recorded_output_once() {
    let call = ToolCall::new("recorded".to_owned(), "read_file", serde_json::json!({}));
    let raw = ToolResult::ok(call.id.clone(), "raw");
    let before = input(HookEvent::AfterTool {
        call: call.clone(),
        result: raw.clone(),
    });
    let response = HookResponse::ToolOutput {
        output: "shaped".into(),
    };
    let output = apply_hook_response(&before.event, &response).unwrap();
    let state = state(
        vec![HookTrace {
            owner: crate::contracts::OperationOwner::Workflow,
            input: before,
            steps: vec![HookStep {
                invocation_id: uuid::Uuid::new_v4(),
                module_id: "budget".into(),
                outcome: HookStepOutcome::Accepted { response },
            }],
            output: Some(output),
        }],
        vec![RecordedToolInvocation {
            owner: crate::contracts::OperationOwner::Workflow,
            call: call.clone(),
            approval_reason: None,
            resolution: ToolCallResolution::Allowed,
            result: ToolResult::ok(call.id.clone(), "shaped"),
            raw_result: Some(raw),
        }],
    );
    let mut actual = call;
    actual.id = "actual".into();
    state.record_tool_requested(&actual).unwrap();
    let result = state.replay_tool_result(&actual.id).unwrap();
    assert_eq!(result.output, "raw");
    assert_eq!(result.call_id, "actual");
    let replay = ReplayHooks::new(state.clone(), vec!["budget".into()]);
    let output = replay
        .apply(input(HookEvent::AfterTool {
            call: actual.clone(),
            result,
        }))
        .await
        .unwrap();
    let HookEvent::AfterTool { result, .. } = output else {
        panic!("tool output")
    };
    assert_eq!(result.output, "shaped");
    assert_eq!(result.call_id, "actual");
    state
        .record_tool_resolved(&actual, &ToolCallResolution::Allowed)
        .unwrap();
    state.record_tool_result(&result).unwrap();
    assert!(state.summary().issues.is_empty());
}

#[tokio::test]
async fn recorded_failure_is_returned_without_executing_any_handler() {
    let failure = crate::model_standard::ModelFailure::new(
        crate::model_standard::ModelFailureKind::ContextWindowExceeded,
        "recorded handler failure",
    );
    let call = ToolCall::new("recorded".to_owned(), "read_file", serde_json::json!({}));
    let before = input(HookEvent::BeforeTool {
        call,
        spec: None,
        blocked: None,
    });
    let state = state(
        vec![HookTrace {
            owner: crate::contracts::OperationOwner::Workflow,
            input: before.clone(),
            steps: vec![HookStep {
                invocation_id: uuid::Uuid::new_v4(),
                module_id: "guard".into(),
                outcome: HookStepOutcome::Failed {
                    model_failure: Some(failure.clone()),
                    message: "recorded handler failure".into(),
                },
            }],
            output: None,
        }],
        vec![],
    );
    let replay = ReplayHooks::new(state.clone(), vec!["guard".into()]);
    let error = replay.apply(before).await.unwrap_err();
    assert_eq!(error.to_string(), "recorded handler failure");
    assert_eq!(
        error.downcast_ref::<crate::model_standard::ModelFailure>(),
        Some(&failure)
    );
    assert!(state.summary().issues.is_empty());
}

#[tokio::test]
async fn notification_failure_is_best_effort_and_missing_boundaries_are_reported() {
    let before = input(HookEvent::TurnStarted {
        task: AgentTask::new("task", ".".into()),
        history: vec![],
    });
    let trace = HookTrace {
        owner: crate::contracts::OperationOwner::Workflow,
        input: before.clone(),
        steps: vec![HookStep {
            invocation_id: uuid::Uuid::new_v4(),
            module_id: "observer".into(),
            outcome: HookStepOutcome::Failed {
                model_failure: None,
                message: "unavailable".into(),
            },
        }],
        output: Some(before.event.clone()),
    };
    let state = state(vec![trace], vec![]);
    let replay = ReplayHooks::new(state.clone(), vec!["observer".into()]);
    assert_eq!(replay.apply(before.clone()).await.unwrap(), before.event);
    assert!(state.summary().issues.is_empty());
    assert!(replay.apply(before).await.is_err());
    assert!(!state.summary().issues.is_empty());
}

#[tokio::test]
async fn recorded_order_must_match_the_snapshot_even_without_workers() {
    let before = input(HookEvent::TurnStarted {
        task: AgentTask::new("task", ".".into()),
        history: vec![],
    });
    let trace = HookTrace {
        owner: crate::contracts::OperationOwner::Workflow,
        input: before.clone(),
        steps: vec![HookStep {
            invocation_id: uuid::Uuid::new_v4(),
            module_id: "different".into(),
            outcome: HookStepOutcome::Accepted {
                response: HookResponse::Continue,
            },
        }],
        output: Some(before.event.clone()),
    };
    let state = state(vec![trace], vec![]);
    let replay = ReplayHooks::new(state.clone(), vec!["configured".into()]);
    assert!(
        replay
            .apply(before)
            .await
            .unwrap_err()
            .to_string()
            .contains("order differs")
    );
    assert!(!state.summary().issues.is_empty());
}

#[test]
fn pending_pre_model_request_is_available_without_a_recorded_exchange() {
    let spec = crate::domain::ToolSpec::new(
        "fixture",
        "fixture",
        serde_json::json!({"type":"object"}),
        crate::domain::ToolSafety::ReadOnly,
    );
    let template = crate::model_standard::CanonicalModelRequest::new(
        crate::domain::ModelRef::new("fake", "fake"),
        vec![],
    )
    .with_tools(vec![spec.clone()]);
    let prepared = input(HookEvent::BeforeContext {
        input: crate::contracts::ContextPreparationInput::new(vec![], Some(template)),
        output: crate::contracts::ContextPreparationOutput::unchanged(vec![]),
        target: None,
    });
    // The review prepares with selected tools, then disables calls before
    // sampling; selector telemetry is attached after preparation as well.
    let mut request = crate::model_standard::CanonicalModelRequest::new(
        crate::domain::ModelRef::new("fake", "fake"),
        vec![],
    );
    request.metadata = serde_json::json!({"tool_exposure":{"selected":1}});
    let before = input(HookEvent::BeforeModel {
        origin: crate::contracts::ModelCallOrigin::Direct,
        request: request.clone(),
    });
    let mut state = state(
        vec![
            HookTrace {
                owner: crate::contracts::OperationOwner::Workflow,
                input: prepared.clone(),
                steps: vec![],
                output: Some(prepared.event),
            },
            HookTrace {
                owner: crate::contracts::OperationOwner::Workflow,
                input: before,
                steps: vec![HookStep {
                    invocation_id: uuid::Uuid::new_v4(),
                    module_id: "guard".into(),
                    outcome: HookStepOutcome::Failed {
                        model_failure: None,
                        message: "stop".into(),
                    },
                }],
                output: None,
            },
        ],
        vec![],
    );
    Arc::get_mut(&mut state)
        .unwrap()
        .registered_tool_names
        .insert(spec.name.clone());
    assert_eq!(state.current_request().unwrap(), request);
    let selection = state.tool_exposure().unwrap();
    assert_eq!(selection.tools, vec![spec]);
    assert_eq!(selection.metadata, request.metadata["tool_exposure"]);
    state.lock().hooks[0].1 = true;
    assert!(state.tool_exposure().unwrap().tools.is_empty());
}

#[tokio::test]
async fn recorded_nested_auxiliary_and_tool_hooks_are_injected_without_calls() {
    use crate::{
        contracts::{ContextPreparationInput, ContextPreparationOutput, OperationOwner},
        core::ModelResponseOutcome,
        model_standard::{CanonicalModelRequest, ModelFailure},
    };
    let invocation = uuid::Uuid::new_v4();
    let call = ToolCall::new("nested", "fixture", serde_json::json!({}));
    let tool_result = ToolResult::ok(call.id.clone(), "recorded nested result");
    let source = crate::model_standard::CanonicalMessage::text(
        crate::model_standard::MessageRole::User,
        "source material",
    );
    let prepared = input(HookEvent::BeforeContext {
        input: ContextPreparationInput::new(vec![source.clone()], None),
        output: ContextPreparationOutput::unchanged(vec![source.clone()]),
        target: None,
    });
    let trace = |owner, event: HookEvent, id| HookTrace {
        owner,
        input: input(event.clone()),
        steps: vec![HookStep {
            invocation_id: id,
            module_id: "fixture".into(),
            outcome: HookStepOutcome::Accepted {
                response: HookResponse::Continue,
            },
        }],
        output: Some(event),
    };
    let owner = OperationOwner::Tool {
        call_id: call.id.clone(),
    };
    let state = state(
        vec![
            trace(
                owner.clone(),
                HookEvent::BeforeTool {
                    call: call.clone(),
                    spec: None,
                    blocked: None,
                },
                uuid::Uuid::new_v4(),
            ),
            trace(
                owner,
                HookEvent::AfterTool {
                    call: call.clone(),
                    result: tool_result.clone(),
                },
                uuid::Uuid::new_v4(),
            ),
            HookTrace {
                owner: OperationOwner::Workflow,
                input: prepared.clone(),
                steps: vec![HookStep {
                    invocation_id: invocation,
                    module_id: "fixture".into(),
                    outcome: HookStepOutcome::Accepted {
                        response: HookResponse::ContextPreparation {
                            output: ContextPreparationOutput::unchanged(vec![source.clone()]),
                        },
                    },
                }],
                output: Some(prepared.event.clone()),
            },
        ],
        vec![RecordedToolInvocation {
            owner: OperationOwner::Hook {
                invocation_id: invocation,
            },
            call: call.clone(),
            approval_reason: None,
            resolution: ToolCallResolution::Allowed,
            result: tool_result,
            raw_result: None,
        }],
    );
    state.set_auxiliary(vec![super::super::super::fixture::RecordedModelExchange {
        owner: OperationOwner::Hook {
            invocation_id: invocation,
        },
        completed_messages: vec![],
        exchange_id: uuid::Uuid::new_v4(),
        request: CanonicalModelRequest::new(crate::domain::ModelRef::new("fake", "fake"), vec![]),
        outcome: ModelResponseOutcome::Error {
            failure: ModelFailure::other("caught auxiliary failure"),
        },
    }]);
    let mut actual = prepared;
    let HookEvent::BeforeContext { input, output, .. } = &mut actual.event else {
        panic!()
    };
    input.messages[0].id = uuid::Uuid::new_v4();
    input.messages[0].parts[0].part_id = uuid::Uuid::new_v4();
    output.messages = input.messages.clone();
    let event = ReplayHooks::new(state.clone(), vec!["fixture".into()])
        .apply(actual.clone())
        .await
        .unwrap();
    assert_eq!(
        event, actual.event,
        "unchanged proposal retains refreshed canonical identities"
    );
    let summary = state.summary();
    assert_eq!(summary.model_exchanges, 0);
    assert_eq!(summary.tool_calls, 1);
    assert!(summary.issues.is_empty(), "{:?}", summary.issues);
}
