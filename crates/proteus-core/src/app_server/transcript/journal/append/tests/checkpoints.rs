use super::*;

fn checkpoint_fixture() -> Vec<JournalRecord> {
    let session = new_session_id();
    let turn = new_turn_id();
    let thread = new_thread_id();
    let execution = crate::domain::new_execution_id();
    let old = text(MessageRole::User, "cached before the turn");
    let user = text(MessageRole::User, "new user message");
    let first = text(MessageRole::Assistant, "first checkpoint answer");
    let final_answer = text(MessageRole::Assistant, "second checkpoint answer");
    let mut records = vec![
        history(1, vec![old.clone()]),
        record(
            2,
            JournalEntry::TurnOpened(TurnOpened {
                task: AgentTask::new("checkpoint fixture", "/synthetic".into()),
                intent: None,
                base_history_revision: 1,
                module_epoch: 0,
                config_snapshot: json!({}),
            }),
        ),
        history(3, vec![user.clone()]),
        history(4, vec![old.clone(), user.clone(), first.clone()]),
        history(5, vec![old, user, first, final_answer]),
        record(
            6,
            JournalEntry::TurnSettled(TurnSettled {
                status: TurnSettlementStatus::Success,
                output: None,
                error: None,
            }),
        ),
    ];
    let mut revision = 0;
    for record in &mut records {
        record.session_id = session;
        if record.session_seq > 1 {
            record.turn_id = Some(turn);
            record.thread_id = Some(thread);
        }
        if matches!(record.entry, JournalEntry::TurnOpened(_)) {
            record.execution_id = Some(execution);
        }
        if let JournalEntry::HistoryMutated(mutation) = &mut record.entry {
            mutation.previous_revision = revision;
            revision += 1;
            mutation.new_revision = revision;
            if record.session_seq >= 4 {
                mutation.mutation = HistoryMutationKind::Checkpoint;
            }
        }
    }
    crate::core::JournalProjection::build(session, records.clone()).unwrap();
    records
}

fn inspect(records: &[JournalRecord], baseline: &[[u8; 32]]) -> Result<Option<usize>> {
    independent_append(
        |visit| {
            for record in records {
                if is_display_kind(record.entry.kind()) {
                    visit(record.clone())?;
                }
            }
            Ok(())
        },
        1,
        baseline,
        4,
    )
}

#[test]
fn validated_new_turn_with_two_retained_history_checkpoints_hits_fast_path() {
    let records = checkpoint_fixture();
    let appended = append_records(&records, 1)
        .unwrap()
        .expect("checkpoint fast hit");
    assert_eq!(appended.conversation_appended, 3);
    let texts = appended
        .transcript
        .iter()
        .map(|row| row.text.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        [
            "new user message",
            "first checkpoint answer",
            "second checkpoint answer"
        ]
    );
    assert_split_equivalence(&records);
}

#[test]
fn a_checkpoint_must_retain_the_exact_canonical_prefix_including_metadata() {
    let records = checkpoint_fixture();
    let baseline = validated_history(&records[..1])
        .unwrap()
        .iter()
        .map(history_message_sha256)
        .collect::<Result<Vec<_>>>()
        .unwrap();
    assert_eq!(inspect(&records, &baseline).unwrap(), Some(3));
    let mut changed = records.clone();
    let JournalEntry::HistoryMutated(checkpoint) = &mut changed[3].entry else {
        unreachable!()
    };
    checkpoint.messages[0].metadata = json!({"changed": true});
    assert!(inspect(&changed, &baseline).unwrap().is_none());
}

#[test]
fn old_identities_in_new_checkpoint_messages_still_require_full_projection() {
    let records = checkpoint_fixture();
    let history = validated_history(&records[..1]).unwrap();
    let baseline = history
        .iter()
        .map(history_message_sha256)
        .collect::<Result<Vec<_>>>()
        .unwrap();
    for message_id in [false, true] {
        let mut changed = records.clone();
        let JournalEntry::HistoryMutated(checkpoint) = &mut changed[3].entry else {
            unreachable!()
        };
        if message_id {
            checkpoint.messages[2].id = history[0].id;
        } else {
            checkpoint.messages[2].parts[0].part_id = history[0].parts[0].part_id;
        }
        assert!(inspect(&changed, &baseline).unwrap().is_none());
    }
}

#[test]
fn a_shortened_checkpoint_falls_back_without_panicking() {
    let mut records = checkpoint_fixture();
    let baseline = validated_history(&records[..1])
        .unwrap()
        .iter()
        .map(history_message_sha256)
        .collect::<Result<Vec<_>>>()
        .unwrap();
    let JournalEntry::HistoryMutated(checkpoint) = &mut records[3].entry else {
        unreachable!()
    };
    checkpoint.messages.clear();
    assert!(inspect(&records, &baseline).unwrap().is_none());
}

#[test]
fn retained_citations_are_stripped_but_new_citations_still_fall_back() {
    let mut records = checkpoint_fixture();
    let old = CanonicalMessage::new(
        MessageRole::Assistant,
        vec![
            ContentPart::HostedToolActivity {
                activity: HostedToolActivity::WebSearch {
                    id: "old-hosted".into(),
                    status: HostedToolStatus::Completed,
                    action: WebSearchAction::Search {
                        queries: vec![],
                        sources: vec![],
                    },
                },
            },
            ContentPart::Citation {
                citation: Citation::Url {
                    start_index: 0,
                    end_index: 1,
                    title: "retained".into(),
                    url: "https://example.com".into(),
                },
            },
        ],
    );
    for record in &mut records {
        if let JournalEntry::HistoryMutated(mutation) = &mut record.entry
            && record.session_seq != 3
        {
            mutation.messages[0] = old.clone();
        }
    }
    crate::core::JournalProjection::build(records[0].session_id, records.clone()).unwrap();
    assert!(append_records(&records, 1).unwrap().is_some());
    assert_split_equivalence(&records);
    let JournalEntry::HistoryMutated(checkpoint) = &mut records[4].entry else {
        unreachable!()
    };
    checkpoint
        .messages
        .last_mut()
        .unwrap()
        .parts
        .push(old.parts[1].clone().with_id(crate::domain::new_part_id()));
    assert!(append_records(&records, 1).unwrap().is_none());
}

#[test]
fn first_explicit_commit_of_a_captured_result_can_reveal_an_old_model_response() {
    use crate::{
        contracts::WorkflowToolResultBinding,
        domain::{ModelRef, ToolCallResolution, new_execution_id},
    };

    let session = new_session_id();
    let first_turn = new_turn_id();
    let next_turn = new_turn_id();
    let first_thread = new_thread_id();
    let next_thread = new_thread_id();
    let first_execution = new_execution_id();
    let next_execution = new_execution_id();
    let exchange = new_exchange_id();
    let call = ToolCall::new("captured", "probe", json!({}));
    let binding = WorkflowToolResultBinding::new(call.clone());
    let result = ToolResult::ok(call.id.clone(), "captured result");
    let captured = binding.message(result.clone());
    let call_message = CanonicalMessage::new(
        MessageRole::Assistant,
        vec![ContentPart::ToolCall { call: call.clone() }],
    );
    let mut hidden = text(MessageRole::Assistant, "late visible text");
    hidden.parts.push(captured.parts[0].clone());
    let open = |revision| {
        JournalEntry::TurnOpened(TurnOpened {
            task: AgentTask::new("captured visibility", "/synthetic".into()),
            intent: None,
            base_history_revision: revision,
            module_epoch: 0,
            config_snapshot: json!({}),
        })
    };
    let settled = || {
        JournalEntry::TurnSettled(TurnSettled {
            status: TurnSettlementStatus::Success,
            output: None,
            error: None,
        })
    };
    let entries = vec![
        open(0),
        JournalEntry::HistoryMutated(HistoryMutated {
            previous_revision: 0,
            new_revision: 1,
            mutation: HistoryMutationKind::Checkpoint,
            messages: vec![call_message.clone()],
            compaction: None,
            tool_results: vec![binding],
        }),
        JournalEntry::ToolCallRecorded(ToolCallRecorded {
            call: call.clone(),
            phase: ToolCallRecordPhase::Requested {
                owner: OperationOwner::Workflow,
            },
        }),
        JournalEntry::ToolCallRecorded(ToolCallRecorded {
            call,
            phase: ToolCallRecordPhase::Resolved {
                resolution: ToolCallResolution::Allowed,
            },
        }),
        JournalEntry::ToolResultRecorded(ToolResultRecorded { result }),
        JournalEntry::ModelRequestRecorded(ModelRequestRecorded {
            exchange_id: exchange,
            origin: ModelCallOrigin::Direct,
            owner: OperationOwner::Workflow,
            request: CanonicalModelRequest::new(ModelRef::new("fake", "fake"), vec![]),
        }),
        JournalEntry::ModelResponseRecorded(ModelResponseRecorded {
            exchange_id: exchange,
            outcome: ModelResponseOutcome::Response {
                response: CanonicalModelResponse::new(hidden, vec![], FinishReason::Stop),
            },
        }),
        settled(),
        open(2),
        JournalEntry::HistoryMutated(HistoryMutated {
            previous_revision: 2,
            new_revision: 3,
            mutation: HistoryMutationKind::Checkpoint,
            messages: vec![call_message, captured],
            compaction: None,
            tool_results: vec![],
        }),
        settled(),
    ];
    let records = entries
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            let mut record = record(index as u64 + 1, entry);
            record.session_id = session;
            let (turn, thread, execution) = if index < 8 {
                (first_turn, first_thread, first_execution)
            } else {
                (next_turn, next_thread, next_execution)
            };
            record.turn_id = Some(turn);
            record.thread_id = Some(thread);
            if !matches!(
                record.entry,
                JournalEntry::HistoryMutated(_) | JournalEntry::TurnSettled(_)
            ) {
                record.execution_id = Some(execution);
            }
            record
        })
        .collect::<Vec<_>>();
    let prefix = crate::core::JournalProjection::build(session, records[..8].to_vec()).unwrap();
    let final_projection = crate::core::JournalProjection::build(session, records.clone()).unwrap();
    assert_eq!(prefix.history_revision, 2);
    assert_eq!(
        prefix.history.len(),
        2,
        "result was captured without a history commit"
    );
    let old_rows = super::super::super::source::from_records(&prefix.records, None).unwrap();
    let new_rows =
        super::super::super::source::from_records(&final_projection.records, None).unwrap();
    assert!(
        !old_rows
            .iter()
            .any(|row| row.text.contains("late visible text"))
    );
    assert!(
        new_rows
            .iter()
            .any(|row| row.text.contains("late visible text"))
    );
    assert!(append_records(&records, 8).unwrap().is_none());
    assert_split_equivalence(&records);
}
