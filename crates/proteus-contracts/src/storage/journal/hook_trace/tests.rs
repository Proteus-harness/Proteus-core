use super::*;
use crate::{
    contracts::{
        ContextPreparationInput, ContextPreparationOutput, ExecutionAttribution, HookEvent,
        HookInput, HookResponse, HookStep, HookStepOutcome, OperationOwner,
    },
    domain::new_execution_id,
    model_standard::{CanonicalMessage, MessageRole},
    storage::journal::{JournalEntry, JournalKind},
};

fn trace() -> HookTrace {
    let messages = vec![CanonicalMessage::text(
        MessageRole::User,
        "large history ".repeat(2000),
    )];
    let output = ContextPreparationOutput::unchanged(messages.clone());
    let event = HookEvent::BeforeContext {
        input: ContextPreparationInput::new(messages, None),
        output: output.clone(),
        model_capabilities: None,
        target: None,
    };
    HookTrace {
        owner: OperationOwner::Workflow,
        input: HookInput {
            event: event.clone(),
            attribution: ExecutionAttribution::detached(new_execution_id()),
            cwd: ".".into(),
            conversation: None,
        },
        steps: vec![HookStep {
            invocation_id: uuid::Uuid::new_v4(),
            module_id: "context".into(),
            outcome: HookStepOutcome::Accepted {
                response: HookResponse::ContextPreparation { output },
            },
        }],
        output: Some(event),
    }
}

#[test]
fn journal_roundtrip_shares_equal_arrays_and_output_without_changing_live_trace_wire() {
    let trace = trace();
    let plain = serde_json::to_value(&trace).unwrap();
    assert!(plain.get("encoding").is_none());
    assert!(plain["input"]["event"]["input"]["messages"].is_array());
    let entry = JournalEntry::HookInvoked(trace.clone());
    assert_eq!(entry.payload_value().unwrap(), plain);
    assert!(
        serde_json::to_value(&entry).unwrap()["payload"]
            .get("encoding")
            .is_none()
    );
    let compact = entry.stored_payload_value().unwrap();
    assert_eq!(compact["encoding"], ENCODING);
    assert!(
        JournalEntry::from_kind_and_payload(JournalKind::HookInvoked, compact.clone()).is_err()
    );
    assert_eq!(compact["messages"].as_array().unwrap().len(), 1);
    assert!(
        serde_json::to_vec(&compact).unwrap().len() < serde_json::to_vec(&plain).unwrap().len() / 3
    );
    assert_eq!(
        JournalEntry::from_stored_kind_and_payload(JournalKind::HookInvoked, compact.clone())
            .unwrap(),
        entry
    );
    assert_eq!(
        JournalEntry::from_stored_kind_and_bytes(
            JournalKind::HookInvoked,
            &serde_json::to_vec(&compact).unwrap()
        )
        .unwrap(),
        entry
    );
    assert_eq!(
        serde_json::from_value::<JournalEntry>(serde_json::to_value(&entry).unwrap()).unwrap(),
        entry
    );
    assert_eq!(
        JournalEntry::from_stored_kind_and_payload(JournalKind::HookInvoked, plain).unwrap(),
        entry
    );
}

#[test]
fn changed_outputs_and_failed_traces_keep_their_complete_evidence() {
    let mut trace = trace();
    if let Some(HookEvent::BeforeContext { output, .. }) = &mut trace.output {
        output.summary = Some("changed result".into());
    }
    let entry = JournalEntry::HookInvoked(trace.clone());
    let value = entry.stored_payload_value().unwrap();
    assert_eq!(value["unchanged_output"], false);
    assert_eq!(
        JournalEntry::from_stored_kind_and_payload(JournalKind::HookInvoked, value).unwrap(),
        entry
    );
    trace.output = None;
    trace.steps.push(HookStep {
        invocation_id: uuid::Uuid::new_v4(),
        module_id: "failed".into(),
        outcome: HookStepOutcome::Failed {
            message: "recorded failure".into(),
            model_failure: None,
        },
    });
    let entry = JournalEntry::HookInvoked(trace);
    assert_eq!(
        JournalEntry::from_stored_kind_and_payload(
            JournalKind::HookInvoked,
            entry.stored_payload_value().unwrap()
        )
        .unwrap(),
        entry
    );
}

#[test]
fn references_reject_out_of_bounds_duplicate_and_non_message_paths() {
    let value = JournalEntry::HookInvoked(trace())
        .stored_payload_value()
        .unwrap();
    let mut invalid = value.clone();
    invalid["references"][0]["index"] = serde_json::json!(9999);
    assert!(JournalEntry::from_stored_kind_and_payload(JournalKind::HookInvoked, invalid).is_err());
    let mut invalid = value.clone();
    let first = invalid["references"][0].clone();
    invalid["references"].as_array_mut().unwrap().push(first);
    assert!(JournalEntry::from_stored_kind_and_payload(JournalKind::HookInvoked, invalid).is_err());
    let mut invalid = value;
    invalid["references"][0]["path"] = serde_json::json!("/input/conversation");
    assert!(JournalEntry::from_stored_kind_and_payload(JournalKind::HookInvoked, invalid).is_err());
}
