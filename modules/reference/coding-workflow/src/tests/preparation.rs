use super::*;
use crate::scaffold::{PersistentRepair, TurnScaffold};
use proteus_contracts::contracts::{
    ContextPreparationTarget, WorkflowCheckpointAck, WorkflowHistoryUpdate,
};

fn candidate(
    input: &WorkflowModuleInput,
) -> (
    Vec<CanonicalMessage>,
    proteus_contracts::domain::HistoryCompactionReport,
) {
    let messages = vec![
        CanonicalMessage::text(MessageRole::User, "summary"),
        input.history.last().unwrap().clone(),
    ];
    let material = ContextPreparationInput::new(input.history.clone(), None);
    let output = proteus_contracts::contracts::ContextPreparationOutput::changed(
        messages.clone(),
        Some("summary".to_owned()),
    );
    (
        messages,
        proteus_contracts::domain::HistoryCompactionReport::from_context_preparation_output(
            &material, &output,
        ),
    )
}

#[test]
fn rejected_preparation_never_escapes_through_failure_or_terminal_output() {
    let input = workflow_input("continue");
    let mut host = FakeHost {
        checkpoint_failure: Some(ProcessModuleError::new("stale revision")),
        ..FakeHost::default()
    };
    let mut turn = TurnScaffold::begin_without_context(&mut host, &input).unwrap();
    let accepted_progress = CanonicalMessage::text(MessageRole::Assistant, "prior progress");
    turn.persistent_messages.push(accepted_progress.clone());
    turn.model_messages.push(accepted_progress.clone());
    let (messages, report) = candidate(&input);
    let target = ContextPreparationTarget {
        execution_id: input.runtime.execution_id,
        revision: 1,
    };
    turn.apply_compaction_report(
        Some(&report),
        Some(&target),
        &messages,
        PersistentRepair::Rebuild,
    )
    .unwrap();
    assert!(turn.finish(&mut host, "done".into(), json!({})).is_err());
    let pending_failure = turn.failure(ProcessModuleError::new("cancelled before checkpoint"));
    assert_eq!(
        pending_failure.history.as_ref().unwrap().new_messages,
        vec![accepted_progress.clone()]
    );
    assert!(
        pending_failure
            .history
            .unwrap()
            .history_replacement
            .is_none()
    );
    assert!(turn.checkpoint(&host, &[]).is_err());
    let failure = turn.failure(ProcessModuleError::new("failed"));
    let history = failure.history.unwrap();
    assert_eq!(history.new_messages, vec![accepted_progress]);
    assert!(history.history_replacement.is_none());
    assert!(!history.compactions.iter().any(|report| report.changed));
    assert_eq!(
        host.checkpoints.lock().unwrap()[0].preparation_target,
        Some(target)
    );
}

#[test]
fn root_acknowledgement_adopts_host_history_and_retains_request_context() {
    let input = workflow_input("continue");
    let mut host = FakeHost::default();
    let mut turn = TurnScaffold::begin(&mut host, &input).unwrap();
    let context = turn.model_messages[0].clone();
    let (mut messages, report) = candidate(&input);
    messages.insert(1, context.clone());
    let target = ContextPreparationTarget {
        execution_id: input.runtime.execution_id,
        revision: 3,
    };
    let steering = CanonicalMessage::text(MessageRole::User, "newly delivered steering");
    let accepted = vec![
        messages[0].clone(),
        input.history[0].clone(),
        steering.clone(),
    ];
    *host.checkpoint_ack.lock().unwrap() = Some(WorkflowCheckpointAck {
        history: WorkflowHistoryUpdate::new(vec![steering])
            .with_history_replacement(accepted[..2].to_vec())
            .with_compactions(vec![report.clone()]),
        messages: accepted.clone(),
        revision: Some(4),
    });
    turn.apply_compaction_report(
        Some(&report),
        Some(&target),
        &messages,
        PersistentRepair::Rebuild,
    )
    .unwrap();
    turn.checkpoint(&host, &[]).unwrap();
    assert_eq!(turn.persistent_messages, accepted);
    assert_eq!(
        turn.model_messages,
        vec![
            messages[0].clone(),
            context,
            messages[2].clone(),
            accepted[2].clone()
        ]
    );
    assert_eq!(
        turn.history_update().unwrap().unwrap().history_replacement,
        Some(accepted[..2].to_vec())
    );
    // A tool can settle while a streaming checkpoint is acknowledged. The
    // accepted result must retire its reserved binding before the later join.
    let call = ToolCall::new("ack-race", "read", json!({}));
    let assistant = CanonicalMessage::new(
        MessageRole::Assistant,
        vec![ContentPart::ToolCall { call: call.clone() }],
    );
    turn.model_messages.push(assistant.clone());
    turn.persistent_messages.push(assistant);
    turn.checkpoint(&host, std::slice::from_ref(&call)).unwrap();
    let binding = host
        .checkpoints
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .tool_results[0]
        .clone();
    let result =
        ToolResult::ok(call.id.clone(), "accepted result").with_metadata(json!({"duration_ms":42}));
    let canonical_result = binding.message(result.clone());
    let mut ack_messages = turn.persistent_messages.clone();
    ack_messages.push(canonical_result.clone());
    let mut progress = turn.history_update().unwrap().unwrap();
    progress.new_messages.push(canonical_result);
    *host.checkpoint_ack.lock().unwrap() = Some(WorkflowCheckpointAck {
        history: progress,
        messages: ack_messages.clone(),
        revision: Some(6),
    });
    turn.checkpoint(&host, &[]).unwrap();
    let mut joined = result.clone();
    joined.metadata["duration_ms"] = json!(0);
    turn.append_tool_results([joined]).unwrap();
    assert_eq!(
        turn.persistent_messages, ack_messages,
        "joined result keeps the acknowledged reserved identity"
    );
    let mut wrong = result;
    wrong.output = "different outcome".into();
    assert!(turn.append_tool_results([wrong]).is_err());
    turn.checkpoint(&host, &[]).unwrap();
    assert!(
        host.checkpoints
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .tool_results
            .is_empty(),
        "completed ACK binding cannot be declared again"
    );
    turn.finish(&mut host, "done".into(), json!({})).unwrap();
}

#[test]
fn detached_acknowledgement_does_not_replace_full_initial_history_with_partial_projection() {
    let mut input = workflow_input("continue");
    let older = CanonicalMessage::text(MessageRole::User, "older input");
    input.history.insert(0, older);
    let mut host = FakeHost::default();
    let mut turn = TurnScaffold::begin_without_context(&mut host, &input).unwrap();
    let progress = CanonicalMessage::text(MessageRole::Assistant, "progress");
    turn.model_messages.push(progress.clone());
    turn.persistent_messages.push(progress);
    let before = turn.persistent_messages.clone();
    turn.checkpoint(&host, &[]).unwrap();
    assert_eq!(turn.persistent_messages, before);
    assert_eq!(turn.model_messages, before);
}
