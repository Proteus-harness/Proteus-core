use proteus_contracts::{
    contracts::{ContextBuildOutcome, ExecutionAttribution, ExecutionRecorder},
    domain::{
        AgentTask, ContextBundle, ContextChunk, ContextRenderMode, new_execution_id, new_record_id,
        new_session_id, new_thread_id, new_turn_id,
    },
};
use serde_json::json;
use uuid::Uuid;

use super::*;
use crate::core::SessionStore;

fn detached_record(
    session_id: crate::domain::SessionId,
    execution_id: crate::domain::ExecutionId,
    session_seq: u64,
    entry: JournalEntry,
) -> JournalRecord {
    JournalRecord {
        schema_version: JOURNAL_SCHEMA_VERSION,
        record_id: new_record_id(),
        session_seq,
        timestamp_ms: session_seq as i64,
        session_id,
        execution_id: Some(execution_id),
        thread_id: None,
        turn_id: None,
        entry,
    }
}

#[test]
fn context_projection_pairs_once_with_exact_owner_and_preserves_pending() {
    let session = new_session_id();
    let execution = new_execution_id();
    let invocation = Uuid::new_v4();
    let request = detached_record(
        session,
        execution,
        1,
        JournalEntry::ContextRequestRecorded(ContextRequestRecorded {
            invocation_id: invocation,
            task: AgentTask::new("context only", "/tmp/workspace".into()),
        }),
    );
    let response = detached_record(
        session,
        execution,
        2,
        JournalEntry::ContextResponseRecorded(ContextResponseRecorded {
            invocation_id: invocation,
            outcome: ContextBuildOutcome::Error {
                message: "provider unavailable".into(),
                causes: Vec::new(),
            },
        }),
    );
    let pending = JournalProjection::build(session, vec![request.clone()]).unwrap();
    assert_eq!(pending.interrupted_context_builds, vec![invocation]);
    let completed =
        JournalProjection::build(session, vec![request.clone(), response.clone()]).unwrap();
    assert!(completed.interrupted_context_builds.is_empty());
    assert!(completed.history.is_empty());
    assert!(completed.interrupted_model_exchanges.is_empty());

    let mut orphan = response.clone();
    orphan.session_seq = 1;
    let mut duplicate_request = request.clone();
    duplicate_request.record_id = new_record_id();
    duplicate_request.session_seq = 2;
    let mut duplicate_response = response.clone();
    duplicate_response.record_id = new_record_id();
    duplicate_response.session_seq = 3;
    let mut wrong_owner = response.clone();
    wrong_owner.execution_id = Some(new_execution_id());
    for (records, expected) in [
        (vec![orphan], "has no preceding request"),
        (
            vec![request.clone(), duplicate_request],
            "duplicate context build request",
        ),
        (
            vec![request.clone(), response, duplicate_response],
            "duplicate context build response",
        ),
        (vec![request, wrong_owner], "changed lifecycle owner"),
    ] {
        let error = JournalProjection::build(session, records).unwrap_err();
        assert!(error.to_string().contains(expected), "{error:#}");
    }
}

#[tokio::test]
async fn context_recorder_persists_exact_task_bundle_error_and_pending_to_cold_projection() {
    let config = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let session = new_session_id();
    let execution = new_execution_id();
    let store = SessionStore::new(config.path(), workspace.path(), session).unwrap();
    let accounting = std::sync::Arc::new(tokio::sync::Mutex::new(
        crate::core::model_context::ModelContextState::default(),
    ));
    let recorder = crate::core::model_context::recorder::ContextExecutionRecorder {
        inner: std::sync::Arc::new(SessionExecutionRecorder::detached(store.clone(), execution)),
        context: accounting.clone(),
    };
    let task = AgentTask::new("inspect context", workspace.path().join("nested"));
    // Exceed the normal blob threshold: the recorded result must survive the
    // same blob storage and canonical reload path as model/tool facts.
    let bundle = ContextBundle::new(vec![
        ContextChunk::new("repository", "x".repeat(DEFAULT_BLOB_THRESHOLD_BYTES + 1))
            .with_path(workspace.path().join("README.md"))
            .with_render_mode(ContextRenderMode::Verbatim)
            .with_score(0.75)
            .with_metadata(json!({"source_revision": 3})),
    ])
    .with_summary("exact summary".into())
    .with_token_estimate(42);
    let successful = Uuid::new_v4();
    let failed = Uuid::new_v4();
    let interrupted = Uuid::new_v4();
    let outcomes = [
        (successful, ContextBuildOutcome::Success { bundle }),
        (
            failed,
            ContextBuildOutcome::Error {
                message: "nested provider error".into(),
                causes: vec!["unavailable".into()],
            },
        ),
    ];
    for (invocation_id, outcome) in &outcomes {
        recorder
            .context_request_recorded(*invocation_id, &task)
            .await
            .unwrap();
        recorder
            .context_response_recorded(*invocation_id, outcome)
            .await
            .unwrap();
    }
    recorder
        .context_request_recorded(interrupted, &task)
        .await
        .unwrap();

    let cold = store.load_projection().unwrap();
    assert_eq!(cold.records.len(), 5);
    assert_eq!(cold.interrupted_context_builds, vec![interrupted]);
    assert!(
        accounting.lock().await.snapshot().is_empty(),
        "context builds do not invent model observations"
    );
    for record in &cold.records {
        assert_eq!(record.execution_id, Some(execution));
        assert!(record.thread_id.is_none() && record.turn_id.is_none());
        match &record.entry {
            JournalEntry::ContextRequestRecorded(request) => assert_eq!(request.task, task),
            JournalEntry::ContextResponseRecorded(response) => {
                let expected = outcomes
                    .iter()
                    .find(|(id, _)| *id == response.invocation_id)
                    .unwrap();
                assert_eq!(response.outcome, expected.1);
            }
            other => panic!("unexpected context record {other:?}"),
        }
    }
    let error = recorder
        .context_response_recorded(successful, &outcomes[0].1)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("duplicate context build response")
    );
    assert_eq!(
        store.load_projection().unwrap().records.len(),
        5,
        "invalid append never commits"
    );
}

#[tokio::test]
async fn context_lifecycle_allows_canceled_and_timeout_turns_with_pending_build() {
    for status in [
        TurnSettlementStatus::Canceled,
        TurnSettlementStatus::Timeout,
    ] {
        let config = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let session = new_session_id();
        let execution = new_execution_id();
        let thread = new_thread_id();
        let turn = new_turn_id();
        let invocation = Uuid::new_v4();
        let task = AgentTask::new("pending context", workspace.path().into());
        let store = SessionStore::new(config.path(), workspace.path(), session).unwrap();
        store
            .append_execution_journal_entry(
                ExecutionAttribution::for_turn(execution, session, thread, turn),
                JournalEntry::TurnOpened(TurnOpened {
                    task: task.clone(),
                    intent: None,
                    base_history_revision: 0,
                    module_epoch: 0,
                    config_snapshot: json!({}),
                }),
            )
            .await
            .unwrap();
        let recorder = SessionExecutionRecorder::for_turn(store.clone(), execution, thread, turn);
        recorder
            .context_request_recorded(invocation, &task)
            .await
            .unwrap();
        store
            .append_journal_entry(
                thread,
                Some(turn),
                JournalEntry::TurnSettled(TurnSettled {
                    status,
                    output: None,
                    error: Some("interrupted".into()),
                }),
            )
            .await
            .unwrap();
        let cold = store.load_projection().unwrap();
        assert_eq!(cold.interrupted_context_builds, vec![invocation]);
        assert!(cold.unsettled_turns.is_empty());
        assert!(
            recorder
                .context_response_recorded(
                    invocation,
                    &ContextBuildOutcome::Error {
                        message: "late result".into(),
                        causes: Vec::new(),
                    }
                )
                .await
                .unwrap_err()
                .to_string()
                .contains("settled root turn")
        );
    }
}
