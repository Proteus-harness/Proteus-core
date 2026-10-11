use super::*;
use crate::{
    contracts::{CancellationToken, ExecutionScope},
    domain::{AgentTask, ContextChunk, new_execution_id, new_message_id},
};

#[test]
fn identical_inputs_consume_distinct_outcomes_in_recorded_request_order() {
    let execution_id = new_execution_id();
    let attribution = ExecutionAttribution::detached(execution_id);
    let task = AgentTask::new("read mutable workspace", ".".into());
    let first = ContextBundle::new(vec![ContextChunk::new("fixture", "before edit")]);
    let second = ContextBundle::new(vec![ContextChunk::new("fixture", "after edit")]);
    let contexts = [first.clone(), second.clone()]
        .into_iter()
        .map(|bundle| RecordedContextBuild {
            invocation_id: uuid::Uuid::new_v4(),
            task: task.clone(),
            outcome: ContextBuildOutcome::Success { bundle },
        })
        .collect();
    let mut state = ReplayState::new(
        vec![],
        vec![],
        vec![],
        vec![],
        contexts,
        &ReasoningConfig::default(),
        new_message_id(),
    );
    state.bind_context(attribution);
    let input = ContextBuildInput::new(
        task,
        ExecutionScope::new(execution_id, CancellationToken::new()),
        attribution,
    );
    assert_eq!(state.context(input.clone()).expect("first read"), first);
    assert_eq!(state.context(input.clone()).expect("second read"), second);
    assert!(state.summary().issues.is_empty());
    assert!(state.context(input).is_err());
    assert!(
        state
            .summary()
            .issues
            .iter()
            .any(|issue| issue.contains("unexpected context build"))
    );
}
