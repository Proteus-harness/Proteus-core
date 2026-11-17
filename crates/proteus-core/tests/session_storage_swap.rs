//! One runtime scenario over two independently selected process-storage exports.
#[path = "support/model.rs"]
mod test_model;

use proteus_contracts::domain::{new_session_id, new_thread_id};
use proteus_core::core::{
    AgentControlSurface, AgentRuntime, AppConfig, JournalEntry, ModuleCatalog, SessionStore,
    TurnSettlementStatus, WorkflowReplayOptions, configured_session_storage, replay_workflow,
};
use serde_json::json;

fn config(backend: &str) -> AppConfig {
    let mut config = test_model::config();
    config.modules.session_storage = Some(backend.into());
    let storage = config.components.get_mut("test-session-storage").unwrap();
    let mut value = serde_json::to_value(&*storage).unwrap();
    value["exports"]["session_storage"] = json!({"jsonl": {}, "sqlite": {}});
    *storage = serde_json::from_value(value).unwrap();
    config.modules.workflow = Some("coding.single_loop".into());
    config.modules.context = Some("simple".into());
    config.modules.policy = Some("allow_all".into());
    config.agent_control.surface = AgentControlSurface::None;
    config.tools.enabled.clear();
    config.components.insert(
        "reference-workflow".into(),
        serde_json::from_value(json!({
            "command": test_model::reference_module(),
            "exports": {
                "workflow": {"coding.single_loop": {}},
                "context": {"simple": {}},
                "policy": {"allow_all": {}}
            }
        }))
        .unwrap(),
    );
    config
}

async fn durable_resume_and_replay(backend: &str) {
    const FIRST: &str = "Remember this first conversation turn";
    const SECOND: &str = "Continue the same conversation after cold resume";
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let config_path = root.path().join("config.toml");
    let config = config(backend);
    let session_id = new_session_id();
    let thread_id = new_thread_id();

    let runtime = AgentRuntime::builder(config.clone(), workspace.clone())
        .with_config_path(Some(&config_path))
        .with_session_ids(session_id, thread_id)
        .build_async()
        .await
        .unwrap();
    let first_output = runtime.run(FIRST.into()).await.unwrap();
    assert!(first_output.text.contains(FIRST));
    let first_history = runtime.history().await.unwrap();
    assert_eq!(first_history.len(), 2);
    let session = runtime.session_dir().unwrap().to_owned();
    drop(runtime);

    // Build a new service, rather than reading from the previous runtime's cache.
    let storage = configured_session_storage(&config, &workspace).unwrap();
    let cold = SessionStore::open(storage, session.clone()).unwrap();
    assert_eq!(cold.session_id(), session_id);
    assert_eq!(cold.load_messages().unwrap(), first_history);
    let first_snapshot = cold.journal_snapshot().unwrap();
    drop(cold);

    let resumed = AgentRuntime::builder(config.clone(), workspace.clone())
        .with_config_path(Some(&config_path))
        .resume_from_session_dir(session.clone(), thread_id)
        .unwrap()
        .build_async()
        .await
        .unwrap();
    assert_eq!(resumed.session_id(), session_id);
    assert_eq!(resumed.history().await.unwrap(), first_history);
    let second_output = resumed.run(SECOND.into()).await.unwrap();
    assert!(second_output.text.contains(SECOND));
    let history = resumed.history().await.unwrap();
    assert_eq!(history.len(), 4);
    assert_eq!(&history[..first_history.len()], first_history.as_slice());
    drop(resumed);

    let storage = configured_session_storage(&config, &workspace).unwrap();
    let cold = SessionStore::open(storage, session.clone()).unwrap();
    let (projection, before) = cold.load_projection_with_snapshot().unwrap();
    assert_eq!(projection.history, history);
    assert!(before.revision.session_seq > first_snapshot.revision.session_seq);
    assert!(before.revision.history_revision > first_snapshot.revision.history_revision);
    let turns = projection
        .records
        .iter()
        .filter_map(|record| {
            if let JournalEntry::TurnSettled(settlement) = &record.entry {
                assert_eq!(settlement.status, TurnSettlementStatus::Success);
                assert_eq!(record.thread_id, Some(thread_id));
                Some(record.turn_id.unwrap())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(turns.len(), 2);
    assert_ne!(turns[0], turns[1]);
    let request = projection
        .records
        .iter()
        .rev()
        .find_map(|record| match &record.entry {
            JournalEntry::ModelRequestRecorded(model) => Some(&model.request),
            _ => None,
        })
        .unwrap();
    for text in [FIRST, first_output.text.as_str(), SECOND] {
        assert!(
            request
                .messages
                .iter()
                .any(|message| message.display_text() == text),
            "{backend}: resumed model request lost conversation message {text:?}"
        );
    }

    let catalog = ModuleCatalog::from_config(&config).unwrap();
    for turn_id in turns {
        let replay = replay_workflow(
            &session,
            &config,
            &catalog,
            WorkflowReplayOptions {
                turn_id: Some(turn_id),
            },
        )
        .await
        .unwrap();
        assert!(
            replay.comparison.matched,
            "{backend}: {:?}",
            replay.comparison.issues
        );
        assert!(replay.source_journal_unchanged);
        assert_eq!(replay.source.session_id, session_id);
        assert_eq!(replay.model_exchanges.recorded, 1);
        assert_eq!(replay.model_exchanges.replayed, 1);
    }
    let after = cold.journal_snapshot().unwrap();
    assert_eq!(before.session, after.session);
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.content_sha256, after.content_sha256);
    assert_eq!(cold.load_messages().unwrap(), history);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jsonl_persists_cold_resumes_and_replays_without_mutation() {
    durable_resume_and_replay("jsonl").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sqlite_persists_cold_resumes_and_replays_without_mutation() {
    durable_resume_and_replay("sqlite").await;
}
