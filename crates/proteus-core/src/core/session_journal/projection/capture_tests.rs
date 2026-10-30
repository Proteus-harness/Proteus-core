use super::super::types::{
    HistoryMutated, ModelRequestRecorded, ToolCallRecorded, ToolResultRecorded, TurnOpened,
    TurnSettled, TurnSettlementStatus,
};
use super::*;
use crate::{
    contracts::{ModelCallOrigin, OperationOwner, WorkflowToolResultBinding},
    domain::{
        AgentTask, ModelRef, ToolCall, ToolCallResolution, ToolResult, new_exchange_id,
        new_execution_id, new_record_id, new_session_id, new_thread_id, new_turn_id,
    },
    model_standard::{CanonicalModelRequest, ContentPart, MessageRole},
};
use serde_json::json;

struct Fixture {
    state: JournalValidationState,
    session: SessionId,
    execution: ExecutionId,
    thread: ThreadId,
    turn: TurnId,
    bindings: Vec<WorkflowToolResultBinding>,
}

impl Fixture {
    fn new() -> Self {
        let mut fixture = Self {
            state: JournalValidationState::default(),
            session: new_session_id(),
            execution: new_execution_id(),
            thread: new_thread_id(),
            turn: new_turn_id(),
            bindings: ["first", "second"]
                .into_iter()
                .map(|id| WorkflowToolResultBinding::new(ToolCall::new(id, "probe", json!({}))))
                .collect(),
        };
        fixture.apply(JournalEntry::TurnOpened(TurnOpened {
            task: AgentTask::new("capture", std::path::PathBuf::from("/tmp/workspace")),
            intent: None,
            base_history_revision: 0,
            module_epoch: 0,
            config_snapshot: json!({}),
        }));
        let history = vec![
            CanonicalMessage::text(MessageRole::User, "unchanged prefix".repeat(1024)),
            CanonicalMessage::new(
                MessageRole::Assistant,
                fixture
                    .bindings
                    .iter()
                    .map(|binding| ContentPart::ToolCall {
                        call: binding.execution_call.clone(),
                    })
                    .collect(),
            ),
        ];
        fixture.apply(JournalEntry::HistoryMutated(HistoryMutated {
            previous_revision: 0,
            new_revision: 1,
            mutation: HistoryMutationKind::Checkpoint,
            messages: history,
            compaction: None,
            tool_results: fixture.bindings.clone(),
        }));
        for binding in fixture.bindings.clone() {
            fixture.apply(JournalEntry::ToolCallRecorded(ToolCallRecorded {
                call: binding.execution_call.clone(),
                phase: ToolCallRecordPhase::Requested {
                    owner: OperationOwner::Workflow,
                },
            }));
            fixture.apply(JournalEntry::ToolCallRecorded(ToolCallRecorded {
                call: binding.execution_call,
                phase: ToolCallRecordPhase::Resolved {
                    resolution: ToolCallResolution::Allowed,
                },
            }));
        }
        fixture
    }

    fn record(&self, entry: JournalEntry) -> JournalRecord {
        let execution_id = match &entry {
            JournalEntry::HistoryMutated(_) | JournalEntry::TurnSettled(_) => None,
            _ => Some(self.execution),
        };
        JournalRecord {
            schema_version: JOURNAL_SCHEMA_VERSION,
            record_id: new_record_id(),
            session_seq: self.state.record_ids.len() as u64 + 1,
            timestamp_ms: 0,
            session_id: self.session,
            execution_id,
            thread_id: Some(self.thread),
            turn_id: Some(self.turn),
            entry,
        }
    }

    fn apply(&mut self, entry: JournalEntry) {
        let record = self.record(entry);
        self.state.apply(&record).unwrap();
    }

    fn completion(&mut self, result: ToolResult, settle: bool) -> JournalRecord {
        if settle {
            self.apply(JournalEntry::ToolEffectRecorded(ToolResultRecorded {
                result,
            }));
            self.record(JournalEntry::TurnSettled(TurnSettled {
                status: TurnSettlementStatus::Canceled,
                output: None,
                error: Some("canceled after effect".into()),
            }))
        } else {
            self.record(JournalEntry::ToolResultRecorded(ToolResultRecorded {
                result,
            }))
        }
    }

    fn append(&mut self, message: CanonicalMessage) {
        let revision = self.state.history_revision;
        self.apply(JournalEntry::HistoryMutated(HistoryMutated {
            previous_revision: revision,
            new_revision: revision + 1,
            mutation: HistoryMutationKind::Append,
            messages: vec![message],
            compaction: None,
            tool_results: vec![],
        }));
    }
}

#[test]
fn capture_reports_the_inserted_index_not_the_last_message() {
    let fixture = Fixture::new();
    let mut capture = fixture.state.capture.as_ref().clone();
    let mut history = fixture.state.history.as_ref().clone();
    let first = ToolResult::ok("first".into(), "first result");
    let second = ToolResult::ok("second".into(), "second result");
    assert_eq!(capture.record_at(&mut history, &second).unwrap(), Some(2));
    assert_eq!(capture.record_at(&mut history, &first).unwrap(), Some(2));
    assert_eq!(history[2], fixture.bindings[0].message(first.clone()));
    assert_eq!(history[3], fixture.bindings[1].message(second));
    let expected = history.clone();
    let undeclared = ToolResult::ok("undeclared".into(), "ignored");
    assert_eq!(capture.record_at(&mut history, &undeclared).unwrap(), None);
    assert!(!capture.record(&mut history, &undeclared).unwrap());
    assert!(capture.record(&mut history, &first).is_err());
    assert_eq!(history, expected);
}

#[test]
fn final_results_and_settled_effects_validate_their_new_parts_in_binding_order() {
    let mut fixture = Fixture::new();
    let prefix = fixture.state.history.as_ref().clone();
    let first = ToolResult::ok("first".into(), "first result");
    let second = ToolResult::ok("second".into(), "second result");
    fixture.apply(JournalEntry::ToolResultRecorded(ToolResultRecorded {
        result: second.clone(),
    }));
    let completion = fixture.completion(first.clone(), true);
    fixture.state.apply(&completion).unwrap();
    assert_eq!(&fixture.state.history[..2], prefix.as_slice());
    assert_eq!(
        &fixture.state.history[2..],
        &[
            fixture.bindings[0].message(first),
            fixture.bindings[1].message(second),
        ]
    );
    assert_eq!(fixture.state.history_revision, 3);
    for message in fixture.state.history.iter() {
        for part in &message.parts {
            assert!(
                fixture
                    .state
                    .known_parts
                    .get(&part.part_id)
                    .unwrap()
                    .matches(part)
            );
        }
    }
    assert_eq!(fixture.state.capture_owner, None);
    assert_eq!(fixture.state.interrupted_turns.len(), 1);
    assert_eq!(
        fixture.state.interrupted_turns[0].after_message_id,
        fixture.state.history[3].id
    );
}

#[test]
fn trace_only_identity_conflicts_reject_both_paths_without_mutating_committed_state() {
    for settle in [false, true] {
        let mut fixture = Fixture::new();
        let result = ToolResult::ok("first".into(), "actual result");
        let mut trace = fixture.bindings[0].message(result.clone());
        trace.parts[0].payload = ContentPart::Text {
            text: "conflicting model trace".into(),
        };
        fixture.apply(JournalEntry::ModelRequestRecorded(ModelRequestRecorded {
            exchange_id: new_exchange_id(),
            origin: ModelCallOrigin::Direct,
            owner: OperationOwner::Workflow,
            request: CanonicalModelRequest::new(ModelRef::new("fake", "model"), vec![trace]),
        }));
        let completion = fixture.completion(result, settle);
        let committed = fixture.state.clone();
        let mut candidate = fixture.state.clone();
        let error = candidate.apply(&completion).unwrap_err().to_string();
        assert_eq!(
            error,
            format!(
                "canonical part {} changed after it was recorded",
                fixture.bindings[0].part_id
            )
        );
        assert_eq!(fixture.state.history, committed.history);
        assert_eq!(fixture.state.history_revision, committed.history_revision);
        assert_eq!(fixture.state.known_parts, committed.known_parts);
        assert!(!fixture.state.record_ids.contains(&completion.record_id));
        assert!(fixture.state.capture.contains_call("first"));
        assert!(!fixture.state.tool_calls.get("first").unwrap().result);
        // Failed COW mutation must not mark the committed binding as recorded.
        let mut capture = fixture.state.capture.as_ref().clone();
        let mut history = fixture.state.history.as_ref().clone();
        assert!(
            capture
                .record(&mut history, &ToolResult::ok("first".into(), "retry"))
                .unwrap()
        );
    }
}

#[test]
fn later_append_id_collisions_keep_active_id_error_precedence() {
    for settle in [false, true] {
        for message_collision in [false, true] {
            let mut fixture = Fixture::new();
            let mut appended = CanonicalMessage::text(MessageRole::User, "later append");
            if message_collision {
                appended.id = fixture.bindings[0].message_id;
            }
            appended.parts[0].part_id = fixture.bindings[0].part_id;
            let appended_id = appended.id;
            fixture.append(appended);
            let completion = fixture.completion(ToolResult::ok("first".into(), "result"), settle);
            let error = fixture
                .state
                .clone()
                .apply(&completion)
                .unwrap_err()
                .to_string();
            let expected = if message_collision {
                format!("active history contains duplicate message id {appended_id}")
            } else {
                format!(
                    "active history contains duplicate part id {} at message {appended_id}",
                    fixture.bindings[0].part_id
                )
            };
            assert_eq!(error, expected);
        }
    }
}
