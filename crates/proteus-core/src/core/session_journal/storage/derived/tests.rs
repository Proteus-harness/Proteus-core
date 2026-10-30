use super::super::{StoredJournalRecord, StoredPayload, write_blob};
use super::*;
use crate::{
    contracts::{ModelCallOrigin, OperationOwner, WorkflowToolResultBinding},
    core::{
        HistoryMutated, HistoryMutationKind, ModelRequestRecorded, ModelResponseOutcome,
        ModelResponseRecorded, ToolCallRecordPhase, ToolCallRecorded, ToolResultRecorded,
        TurnOpened, TurnSettled, TurnSettlementStatus, model_context::ModelContextState,
    },
    domain::{
        AgentTask, HistoryCompactionReport, ModelRef, ToolCall, ToolCallResolution, ToolResult,
        new_exchange_id, new_execution_id, new_record_id, new_session_id, new_thread_id,
        new_turn_id,
    },
    model_standard::{
        CanonicalModelRequest, CanonicalModelResponse, ContentPart, FinishReason, MessageRole,
        ModelFailure, ModelFailureKind, TokenUsage,
    },
};

struct Fixture {
    session: SessionId,
    thread: ThreadId,
    turn: TurnId,
    execution: ExecutionId,
    records: Vec<JournalRecord>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            session: new_session_id(),
            thread: new_thread_id(),
            turn: new_turn_id(),
            execution: new_execution_id(),
            records: Vec::new(),
        }
    }

    fn push(&mut self, entry: JournalEntry, execution: bool, turn: bool) {
        self.records.push(JournalRecord {
            schema_version: JOURNAL_SCHEMA_VERSION,
            record_id: new_record_id(),
            session_seq: self.records.len() as u64 + 1,
            timestamp_ms: 1000 - self.records.len() as i64,
            session_id: self.session,
            execution_id: execution.then_some(self.execution),
            thread_id: Some(self.thread),
            turn_id: turn.then_some(self.turn),
            entry,
        });
    }

    fn revision(&self) -> u64 {
        JournalProjection::build(self.session, self.records.clone())
            .unwrap()
            .history_revision
    }

    fn history(
        &mut self,
        mutation: HistoryMutationKind,
        messages: Vec<CanonicalMessage>,
        compacted: bool,
        turn: bool,
    ) {
        let revision = self.revision();
        let mut compaction = HistoryCompactionReport::unchanged(0, None);
        compaction.changed = true;
        self.push(
            JournalEntry::HistoryMutated(HistoryMutated {
                previous_revision: revision,
                new_revision: revision + 1,
                mutation,
                messages,
                compaction: compacted.then_some(compaction),
                tool_results: Vec::new(),
            }),
            false,
            turn,
        );
    }

    fn request(&mut self, origin: ModelCallOrigin) -> crate::domain::ExchangeId {
        let exchange_id = new_exchange_id();
        let mut request = CanonicalModelRequest::new(
            ModelRef::new("fixture", "model"),
            vec![CanonicalMessage::text(
                MessageRole::User,
                "request payload not retained by the derived fold ".repeat(4096),
            )],
        );
        request.limits.max_input_tokens = Some(100_000);
        self.push(
            JournalEntry::ModelRequestRecorded(ModelRequestRecorded {
                exchange_id,
                origin,
                owner: OperationOwner::Workflow,
                request,
            }),
            true,
            true,
        );
        exchange_id
    }
}

fn stored(record: &JournalRecord) -> StoredJournalRecord {
    StoredJournalRecord {
        schema_version: record.schema_version,
        record_id: record.record_id,
        session_seq: record.session_seq,
        timestamp_ms: record.timestamp_ms,
        session_id: record.session_id,
        execution_id: record.execution_id,
        thread_id: record.thread_id,
        turn_id: record.turn_id,
        kind: record.entry.kind(),
        payload: StoredPayload::Inline {
            value: record.entry.payload_value().unwrap(),
        },
    }
}

fn write_stored(dir: &Path, records: &[StoredJournalRecord], tail: &[u8]) {
    let mut bytes = Vec::new();
    for record in records {
        serde_json::to_writer(&mut bytes, record).unwrap();
        bytes.push(b'\n');
    }
    bytes.extend_from_slice(tail);
    std::fs::write(journal_path(dir), bytes).unwrap();
}

fn reference(dir: &Path, session: SessionId) -> Result<JournalProjection> {
    JournalProjection::build(session, read::load_records(dir, session)?)
}

fn assert_derived(dir: &Path, session: SessionId) {
    let projection = reference(dir, session).unwrap();
    let mut usage = UsageProjection::default();
    for record in &projection.records {
        usage.apply(record);
    }
    let conversation = ConversationSnapshot {
        revision: projection.history_revision,
        messages: projection.history.clone(),
        model_context: ModelContextState::from_records(&projection.records, None).snapshot(),
    };
    assert_eq!(
        load_validated_history(dir, session).unwrap(),
        projection.history
    );
    assert_eq!(
        load_validated_history_revision(dir, session).unwrap(),
        projection.history_revision,
    );
    assert_eq!(
        load_validated_conversation(dir, session).unwrap(),
        conversation
    );
    assert_eq!(
        load_validated_usage(dir, session).unwrap(),
        usage.snapshot(session)
    );
}

fn assert_same_error(dir: &Path, session: SessionId, expected: &str) {
    let reference = format!("{:#}", reference(dir, session).unwrap_err());
    assert!(reference.contains(expected), "{reference}");
    let errors = [
        format!("{:#}", load_validated_history(dir, session).unwrap_err()),
        format!(
            "{:#}",
            load_validated_history_revision(dir, session).unwrap_err()
        ),
        format!(
            "{:#}",
            load_validated_conversation(dir, session).unwrap_err()
        ),
        format!("{:#}", load_validated_usage(dir, session).unwrap_err()),
    ];
    for error in errors {
        assert_eq!(error, reference);
    }
}

#[test]
fn derived_reads_match_full_projection_at_every_prefix_and_both_encodings() {
    let mut fixture = Fixture::new();
    let user = CanonicalMessage::text(MessageRole::User, "retained user");
    fixture.history(
        HistoryMutationKind::Append,
        vec![user.clone()],
        false,
        false,
    );
    fixture.push(
        JournalEntry::TurnOpened(TurnOpened {
            task: AgentTask::new("derived reads", "/tmp/workspace".into()),
            intent: None,
            base_history_revision: fixture.revision(),
            module_epoch: 0,
            config_snapshot: serde_json::json!({}),
        }),
        true,
        true,
    );
    let completed = fixture.request(ModelCallOrigin::Direct);
    let answer = CanonicalMessage::text(MessageRole::Assistant, "answer");
    fixture.push(
        JournalEntry::ModelResponseRecorded(ModelResponseRecorded {
            exchange_id: completed,
            outcome: ModelResponseOutcome::Response {
                response: CanonicalModelResponse::new(
                    answer.clone(),
                    Vec::new(),
                    FinishReason::Stop,
                )
                .with_usage(TokenUsage::new(10, 2)),
            },
        }),
        true,
        true,
    );
    fixture.history(
        HistoryMutationKind::Checkpoint,
        vec![user.clone(), answer.clone()],
        false,
        true,
    );
    let failed = fixture.request(ModelCallOrigin::Direct);
    fixture.push(
        JournalEntry::ModelResponseRecorded(ModelResponseRecorded {
            exchange_id: failed,
            outcome: ModelResponseOutcome::Error {
                failure: ModelFailure::new(ModelFailureKind::ContextWindowExceeded, "overflow"),
            },
        }),
        true,
        true,
    );
    fixture.history(HistoryMutationKind::Replace, vec![user, answer], true, true);
    fixture.request(ModelCallOrigin::Auxiliary);
    fixture.push(
        JournalEntry::TurnSettled(TurnSettled {
            status: TurnSettlementStatus::Canceled,
            output: None,
            error: Some("cancelled".into()),
        }),
        false,
        true,
    );
    fixture.history(
        HistoryMutationKind::Replace,
        vec![CanonicalMessage::text(
            MessageRole::User,
            "new conversation",
        )],
        false,
        false,
    );
    for blobs in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut records = fixture.records.iter().map(stored).collect::<Vec<_>>();
        if blobs {
            for record in &mut records {
                let StoredPayload::Inline { value } = &record.payload else {
                    unreachable!()
                };
                record.payload =
                    write_blob(root.path(), &serde_json::to_vec(value).unwrap()).unwrap();
            }
        }
        for end in 0..=records.len() {
            write_stored(root.path(), &records[..end], b"{torn tail");
            let before = std::fs::read(journal_path(root.path())).unwrap();
            assert_derived(root.path(), fixture.session);
            assert_eq!(std::fs::read(journal_path(root.path())).unwrap(), before);
        }
    }
}

#[test]
fn captured_results_and_settled_effects_match_full_history_at_each_prefix() {
    for settle_effect in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut fixture = Fixture::new();
        fixture.push(
            JournalEntry::TurnOpened(TurnOpened {
                task: AgentTask::new("capture", "/tmp/workspace".into()),
                intent: None,
                base_history_revision: 0,
                module_epoch: 0,
                config_snapshot: serde_json::json!({}),
            }),
            true,
            true,
        );
        let binding = WorkflowToolResultBinding::new(ToolCall::new(
            "captured",
            "probe",
            serde_json::json!({}),
        ));
        fixture.push(
            JournalEntry::HistoryMutated(HistoryMutated {
                previous_revision: 0,
                new_revision: 1,
                mutation: HistoryMutationKind::Checkpoint,
                messages: vec![CanonicalMessage::new(
                    MessageRole::Assistant,
                    vec![ContentPart::ToolCall {
                        call: binding.execution_call.clone(),
                    }],
                )],
                compaction: None,
                tool_results: vec![binding.clone()],
            }),
            false,
            true,
        );
        for phase in [
            ToolCallRecordPhase::Requested {
                owner: OperationOwner::Workflow,
            },
            ToolCallRecordPhase::Resolved {
                resolution: ToolCallResolution::Allowed,
            },
        ] {
            fixture.push(
                JournalEntry::ToolCallRecorded(ToolCallRecorded {
                    call: binding.execution_call.clone(),
                    phase,
                }),
                true,
                true,
            );
        }
        let mut result = ToolResult::ok(binding.call_id.clone(), "effect payload ".repeat(4096));
        fixture.push(
            JournalEntry::ToolEffectRecorded(ToolResultRecorded {
                result: result.clone(),
            }),
            true,
            true,
        );
        if !settle_effect {
            result.output = "formatted final result".into();
            fixture.push(
                JournalEntry::ToolResultRecorded(ToolResultRecorded {
                    result: result.clone(),
                }),
                true,
                true,
            );
        }
        fixture.push(
            JournalEntry::TurnSettled(TurnSettled {
                status: TurnSettlementStatus::Canceled,
                output: None,
                error: Some("stopped after effect".into()),
            }),
            false,
            true,
        );
        let records = fixture.records.iter().map(stored).collect::<Vec<_>>();
        for end in 0..=records.len() {
            write_stored(root.path(), &records[..end], b"");
            assert_derived(root.path(), fixture.session);
        }
        assert_eq!(
            load_validated_history(root.path(), fixture.session)
                .unwrap()
                .last(),
            Some(&binding.message(result)),
        );
        assert_eq!(
            load_validated_history_revision(root.path(), fixture.session).unwrap(),
            2
        );
    }
}

#[test]
fn later_decode_failures_precede_the_first_semantic_error() {
    let root = tempfile::tempdir().unwrap();
    let mut fixture = Fixture::new();
    fixture.history(
        HistoryMutationKind::Append,
        vec![CanonicalMessage::text(MessageRole::User, "first")],
        false,
        false,
    );
    let mut first = stored(&fixture.records[0]);
    first.session_seq = 2;
    // The first semantic error must not prevent scanning a later stored record.
    write_stored(root.path(), &[first.clone()], b"{malformed}\n");
    assert_same_error(root.path(), fixture.session, "failed to parse journal");
    write_stored(root.path(), &[first.clone()], b"\n");
    assert_same_error(root.path(), fixture.session, "contains an empty line");
    write_stored(root.path(), &[first.clone()], b"{ignored torn tail");
    assert_same_error(root.path(), fixture.session, "sequence mismatch");

    let mut later = stored(&fixture.records[0]);
    later.record_id = new_record_id();
    later.session_seq = 3;
    later.schema_version += 1;
    write_stored(root.path(), &[first.clone(), later.clone()], b"");
    assert_same_error(
        root.path(),
        fixture.session,
        "unsupported journal schema_version",
    );
    later.schema_version = JOURNAL_SCHEMA_VERSION;
    later.session_id = new_session_id();
    write_stored(root.path(), &[first.clone(), later.clone()], b"");
    assert_same_error(root.path(), fixture.session, "belongs to session");
    later.session_id = fixture.session;
    later.payload = StoredPayload::Inline {
        value: serde_json::json!({}),
    };
    write_stored(root.path(), &[first.clone(), later.clone()], b"");
    assert_same_error(root.path(), fixture.session, "invalid journal payload");

    let payload = serde_json::to_vec(&fixture.records[0].entry.payload_value().unwrap()).unwrap();
    later.payload = write_blob(root.path(), &payload).unwrap();
    let StoredPayload::Blob { relative_path, .. } = &later.payload else {
        unreachable!()
    };
    let path = root.path().join(relative_path);
    let mut corrupt = payload;
    corrupt[0] ^= 1;
    std::fs::write(&path, &corrupt).unwrap();
    write_stored(root.path(), &[first.clone(), later.clone()], b"");
    assert_same_error(root.path(), fixture.session, "hash mismatch");
    assert_eq!(std::fs::read(&path).unwrap(), corrupt);
    std::fs::remove_file(&path).unwrap();
    assert_same_error(
        root.path(),
        fixture.session,
        "failed to inspect journal blob",
    );

    // With hydration successful, preserve the first semantic error, not a later one.
    later = stored(&fixture.records[0]);
    write_stored(root.path(), &[first, later], b"");
    assert_same_error(root.path(), fixture.session, "sequence mismatch");
}

#[test]
fn a_semantically_invalid_record_never_reaches_the_usage_fold() {
    let root = tempfile::tempdir().unwrap();
    let mut fixture = Fixture::new();
    fixture.push(
        JournalEntry::ModelResponseRecorded(ModelResponseRecorded {
            exchange_id: new_exchange_id(),
            outcome: ModelResponseOutcome::Error {
                failure: ModelFailure::other("no request"),
            },
        }),
        true,
        false,
    );
    // A detached response is well-formed storage, but invalid without a request.
    // UsageProjection::apply assumes validation and would panic on this input.
    fixture.records[0].thread_id = None;
    write_stored(root.path(), &[stored(&fixture.records[0])], b"");
    assert_same_error(root.path(), fixture.session, "has no preceding request");

    fixture.records.clear();
    fixture.history(
        HistoryMutationKind::Append,
        vec![CanonicalMessage::text(MessageRole::User, "first")],
        false,
        false,
    );
    let first = stored(&fixture.records[0]);
    let mut duplicate = first.clone();
    duplicate.session_seq = 2;
    write_stored(root.path(), &[first, duplicate], b"");
    assert_same_error(root.path(), fixture.session, "duplicate journal record_id");
}

#[test]
fn empty_and_invalid_file_reads_do_not_materialize_or_repair_storage() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing");
    let session = new_session_id();
    assert_derived(&missing, session);
    assert!(!missing.exists());
    std::fs::create_dir(journal_path(root.path())).unwrap();
    assert_same_error(root.path(), session, "not a regular file");
    assert!(journal_path(root.path()).is_dir());
}
