//! Compare every validated prefix with the unchanged from-zero replay path.
use super::*;
use crate::{
    domain::{Citation, HostedToolActivity, HostedToolStatus, ImageRef, WebSearchAction},
    model_standard::CanonicalPart,
};

struct Fixture {
    session: crate::domain::SessionId,
    thread: ThreadId,
    turn: TurnId,
    execution: crate::domain::ExecutionId,
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

    fn projection(&self) -> JournalProjection {
        JournalProjection::build(self.session, self.records.clone()).unwrap()
    }

    fn push(&mut self, entry: JournalEntry, execution: bool, turn: bool) {
        let mut item = record(
            self.session,
            execution.then_some(self.execution),
            self.thread,
            self.turn,
            self.records.len() as u64 + 1,
            entry,
        );
        item.turn_id = turn.then_some(self.turn);
        self.records.push(item);
    }

    fn open(&mut self) {
        let base_history_revision = self.projection().history_revision;
        self.push(
            JournalEntry::TurnOpened(TurnOpened {
                intent: None,
                task: AgentTask::new("frontier", "/tmp/workspace".into()),
                base_history_revision,
                module_epoch: 0,
                config_snapshot: json!({}),
            }),
            true,
            true,
        );
    }

    fn history(
        &mut self,
        mutation: HistoryMutationKind,
        messages: Vec<CanonicalMessage>,
        compacted: bool,
        turn: bool,
    ) {
        let previous_revision = self.projection().history_revision;
        let mut compaction = crate::domain::HistoryCompactionReport::unchanged(0, None);
        compaction.changed = true;
        self.push(
            JournalEntry::HistoryMutated(HistoryMutated {
                previous_revision,
                new_revision: previous_revision + 1,
                mutation,
                messages,
                compaction: compacted.then_some(compaction),
                tool_results: vec![],
            }),
            false,
            turn,
        );
    }

    fn assert_prefixes(&self, live_turns: &[Option<TurnId>]) {
        for end in 0..=self.records.len() {
            let projection =
                JournalProjection::build(self.session, self.records[..end].to_vec()).unwrap();
            for &live in live_turns {
                assert_eq!(
                    serde_json::to_value(journal_transcript_messages(
                        projection.records.clone(),
                        live
                    ))
                    .unwrap(),
                    serde_json::to_value(replay_transcript(&projection, live, 0)).unwrap(),
                    "prefix {end}, live {live:?}, frontier {}",
                    replay_start(&projection.records, live),
                );
            }
        }
    }
}

fn image(id: &str) -> ImageRef {
    ImageRef {
        id: id.into(),
        name: format!("{id}.png"),
        mime_type: "image/png".into(),
        path: format!("/session/images/{id}").into(),
    }
}

#[test]
fn reset_frontier_skips_discarded_rows_but_not_first_part_dates() {
    let mut fixture = Fixture::new();
    let old = CanonicalMessage::text(MessageRole::User, "retained part");
    fixture.history(HistoryMutationKind::Append, vec![old.clone()], false, false);
    fixture.records.last_mut().unwrap().timestamp_ms = 100;
    for index in 0..32 {
        fixture.history(
            HistoryMutationKind::Append,
            vec![CanonicalMessage::text(
                MessageRole::Assistant,
                format!("discard {index}"),
            )],
            false,
            false,
        );
    }
    let mut rehosted = CanonicalMessage::text(MessageRole::Assistant, "new part");
    rehosted.parts.insert(0, old.parts[0].clone());
    let reset = fixture.records.len();
    fixture.history(HistoryMutationKind::Replace, vec![rehosted], false, false);
    fixture.records.last_mut().unwrap().timestamp_ms = 50;
    fixture.assert_prefixes(&[None, Some(fixture.turn)]);

    let projection = fixture.projection();
    assert_eq!(replay_start(&projection.records, None), reset);
    assert_eq!(projection.records[reset..].len(), 1);
    let rows = journal_transcript_messages(projection.records.clone(), None);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].text, "retained part\nnew part");
    assert_eq!(rows[0].timestamp_ms, Some(100));
}

#[test]
fn a_live_replace_is_not_a_render_barrier() {
    let mut fixture = Fixture::new();
    fixture.history(
        HistoryMutationKind::Append,
        vec![CanonicalMessage::text(MessageRole::User, "before")],
        false,
        false,
    );
    let settled_reset = fixture.records.len();
    fixture.history(
        HistoryMutationKind::Replace,
        vec![CanonicalMessage::text(MessageRole::User, "settled")],
        false,
        false,
    );
    fixture.open();
    let live_reset = fixture.records.len();
    fixture.history(
        HistoryMutationKind::Replace,
        vec![CanonicalMessage::text(
            MessageRole::User,
            "live replacement",
        )],
        false,
        true,
    );
    fixture.assert_prefixes(&[None, Some(fixture.turn), Some(new_turn_id())]);
    let projection = fixture.projection();
    assert_eq!(
        replay_start(&projection.records, Some(fixture.turn)),
        settled_reset
    );
    assert_eq!(replay_start(&projection.records, None), live_reset);
    assert_eq!(
        journal_transcript_messages(projection.records.clone(), Some(fixture.turn))[0].text,
        "settled"
    );
}

#[test]
fn compaction_is_not_a_barrier_and_citations_and_images_still_rewrite_rows() {
    let mut fixture = Fixture::new();
    fixture.history(
        HistoryMutationKind::Append,
        vec![CanonicalMessage::text(MessageRole::User, "discarded")],
        false,
        false,
    );
    let hosted = CanonicalMessage::new(
        MessageRole::Assistant,
        vec![ContentPart::HostedToolActivity {
            activity: HostedToolActivity::WebSearch {
                id: "hosted".into(),
                status: HostedToolStatus::Completed,
                action: WebSearchAction::Search {
                    queries: vec!["docs".into()],
                    sources: vec![],
                },
            },
        }],
    );
    let mut illustrated = CanonicalMessage::new(
        MessageRole::Assistant,
        vec![
            ContentPart::Text {
                text: "illustrated".into(),
            },
            ContentPart::Image {
                image: image("old"),
            },
        ],
    );
    let reset = fixture.records.len();
    fixture.history(
        HistoryMutationKind::Replace,
        vec![hosted.clone(), illustrated.clone()],
        false,
        false,
    );
    illustrated.parts.push(CanonicalPart::new(
        PartProvenance::Model,
        PartScope::Conversation,
        ContentPart::Image {
            image: image("new"),
        },
    ));
    let mut summary = CanonicalMessage::text(MessageRole::Assistant, "hidden compacted summary");
    summary.parts[0].provenance = PartProvenance::Compactor;
    fixture.history(
        HistoryMutationKind::Replace,
        vec![hosted, illustrated, summary],
        true,
        false,
    );
    fixture.history(
        HistoryMutationKind::Append,
        vec![CanonicalMessage::new(
            MessageRole::Assistant,
            vec![ContentPart::Citation {
                citation: Citation::Url {
                    start_index: 0,
                    end_index: 1,
                    title: "docs".into(),
                    url: "https://example.com/docs".into(),
                },
            }],
        )],
        false,
        false,
    );
    fixture.assert_prefixes(&[None]);
    let projection = fixture.projection();
    assert_eq!(replay_start(&projection.records, None), reset);
    let rows = journal_transcript_messages(projection.records.clone(), None);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].tool.as_ref().unwrap().metadata["citations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // Only the newly admitted image part is rendered; the existing helper
    // replaces the last matching row's image list rather than adding a row.
    assert_eq!(rows[1].images, vec![image("new")]);
    assert_eq!(rows[1].text, "illustrated");
}

#[test]
fn roots_lifecycle_dates_and_final_results_survive_the_render_frontier() {
    let mut fixture = Fixture::new();
    fixture.open();
    let call = ToolCall::new("before-reset", "probe", json!({}));
    fixture.push(
        JournalEntry::ToolCallRecorded(ToolCallRecorded {
            call: call.clone(),
            phase: ToolCallRecordPhase::Requested {
                owner: crate::contracts::OperationOwner::Workflow,
            },
        }),
        true,
        true,
    );
    let first_call_date = fixture.records.last().unwrap().timestamp_ms;
    fixture.push(
        JournalEntry::ToolCallRecorded(ToolCallRecorded {
            call: call.clone(),
            phase: ToolCallRecordPhase::Resolved {
                resolution: ToolCallResolution::Allowed,
            },
        }),
        true,
        true,
    );
    let reset = fixture.records.len();
    fixture.history(HistoryMutationKind::Replace, vec![], false, false);
    let effect =
        ToolResult::ok(call.id.clone(), "effect").with_metadata(json!({"stage": "effect"}));
    fixture.push(
        JournalEntry::ToolEffectRecorded(ToolResultRecorded {
            result: effect.clone(),
        }),
        true,
        true,
    );
    let mut final_result = effect;
    final_result.output = "final".into();
    final_result.metadata = json!({"stage": "final"});
    fixture.push(
        JournalEntry::ToolResultRecorded(ToolResultRecorded {
            result: final_result,
        }),
        true,
        true,
    );
    fixture.push(
        JournalEntry::TurnSettled(TurnSettled {
            status: TurnSettlementStatus::Timeout,
            output: None,
            error: None,
        }),
        false,
        true,
    );
    fixture.assert_prefixes(&[None, Some(fixture.turn)]);
    let projection = fixture.projection();
    assert_eq!(replay_start(&projection.records, None), reset);
    let rows = journal_transcript_messages(projection.records.clone(), None);
    assert_eq!(rows.len(), 2);
    let tool = rows[0].tool.as_ref().unwrap();
    assert_eq!(tool.call_id, call.id);
    assert_eq!(tool.metadata["stage"], "final");
    assert_eq!(rows[0].timestamp_ms, Some(first_call_date));
    assert_eq!(rows[1].text, "AppServer timeout: turn timed out");
}

#[test]
fn a_skipped_history_part_can_still_make_a_later_response_visible() {
    let mut fixture = Fixture::new();
    fixture.open();
    let message = CanonicalMessage::text(MessageRole::Assistant, "old committed identity");
    fixture.history(
        HistoryMutationKind::Append,
        vec![message.clone()],
        false,
        false,
    );
    let first_date = fixture.records.last().unwrap().timestamp_ms;
    let reset = fixture.records.len();
    fixture.history(HistoryMutationKind::Replace, vec![], false, false);
    let exchange = new_exchange_id();
    fixture.push(
        JournalEntry::ModelRequestRecorded(ModelRequestRecorded {
            exchange_id: exchange,
            origin: ModelCallOrigin::Direct,
            owner: crate::contracts::OperationOwner::Workflow,
            request: CanonicalModelRequest::new(ModelRef::new("fake", "model"), vec![]),
        }),
        true,
        true,
    );
    fixture.push(
        JournalEntry::ModelResponseRecorded(ModelResponseRecorded {
            exchange_id: exchange,
            outcome: ModelResponseOutcome::Response {
                response: CanonicalModelResponse::new(message, vec![], FinishReason::Stop),
            },
        }),
        true,
        true,
    );
    fixture.assert_prefixes(&[None, Some(fixture.turn)]);
    let projection = fixture.projection();
    assert_eq!(replay_start(&projection.records, None), reset);
    let rows = journal_transcript_messages(projection.records.clone(), None);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].text, "old committed identity");
    assert_eq!(rows[0].timestamp_ms, Some(first_date));
}

#[test]
fn an_invalid_prefix_is_not_made_valid_by_a_later_reset() {
    let mut fixture = Fixture::new();
    fixture.history(
        HistoryMutationKind::Append,
        vec![CanonicalMessage::text(MessageRole::User, "old")],
        false,
        false,
    );
    fixture.history(HistoryMutationKind::Replace, vec![], false, false);
    fixture.records[0].session_seq = 10;
    assert!(JournalProjection::build(fixture.session, fixture.records).is_err());
}

#[test]
fn a_late_commit_does_not_resurrect_a_response_before_the_reset() {
    let mut fixture = Fixture::new();
    fixture.open();
    let message = CanonicalMessage::text(MessageRole::Assistant, "late committed text");
    let exchange = new_exchange_id();
    fixture.push(
        JournalEntry::ModelRequestRecorded(ModelRequestRecorded {
            exchange_id: exchange,
            origin: ModelCallOrigin::Direct,
            owner: crate::contracts::OperationOwner::Workflow,
            request: CanonicalModelRequest::new(ModelRef::new("fake", "model"), vec![]),
        }),
        true,
        true,
    );
    fixture.push(
        JournalEntry::ModelResponseRecorded(ModelResponseRecorded {
            exchange_id: exchange,
            outcome: ModelResponseOutcome::Response {
                response: CanonicalModelResponse::new(message.clone(), vec![], FinishReason::Stop),
            },
        }),
        true,
        true,
    );
    let first_date = fixture.records.last().unwrap().timestamp_ms;
    let reset = fixture.records.len();
    fixture.history(HistoryMutationKind::Replace, vec![], false, false);
    fixture.history(HistoryMutationKind::Append, vec![message], false, false);
    fixture.assert_prefixes(&[None, Some(fixture.turn)]);
    let projection = fixture.projection();
    assert_eq!(replay_start(&projection.records, None), reset);
    let rows = journal_transcript_messages(projection.records.clone(), None);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].text, "late committed text");
    assert_eq!(rows[0].timestamp_ms, Some(first_date));
}

#[test]
fn reset_does_not_allow_reusing_an_exchange_identity() {
    let mut fixture = Fixture::new();
    fixture.open();
    let exchange = new_exchange_id();
    fixture.push(
        JournalEntry::ModelRequestRecorded(ModelRequestRecorded {
            exchange_id: exchange,
            origin: ModelCallOrigin::Direct,
            owner: crate::contracts::OperationOwner::Workflow,
            request: CanonicalModelRequest::new(ModelRef::new("fake", "model"), vec![]),
        }),
        true,
        true,
    );
    let response = JournalEntry::ModelResponseRecorded(ModelResponseRecorded {
        exchange_id: exchange,
        outcome: ModelResponseOutcome::Response {
            response: CanonicalModelResponse::new(
                CanonicalMessage::text(MessageRole::Assistant, "first response"),
                vec![],
                FinishReason::Stop,
            ),
        },
    });
    fixture.push(response.clone(), true, true);
    fixture.history(HistoryMutationKind::Replace, vec![], false, false);
    fixture.assert_prefixes(&[None, Some(fixture.turn)]);
    fixture.push(response, true, true);
    let error = JournalProjection::build(fixture.session, fixture.records)
        .expect_err("a display reset does not reset exchange validation");
    assert!(
        error
            .to_string()
            .contains("duplicate model exchange response")
    );
}
