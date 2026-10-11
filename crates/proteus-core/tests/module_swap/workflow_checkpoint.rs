use super::*;
use proteus_core::core::{
    HistoryMutationKind, JournalEntry, SessionStore, WorkflowReplayOptions, replay_workflow,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_and_python_workflows_share_the_checkpoint_contract() {
    for workflow in [
        "coding.codex_loop",
        "coding.single_loop",
        "coding.plan_execute_review",
        "python_agent_loop",
    ] {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let config_path = root.path().join("config.json");
        let mut config = test_model::config();
        config.modules.workflow = Some(workflow.into());
        config.modules.context = Some("simple".into());
        config.modules.policy = Some("allow_all".into());
        let mut reference = json!({
            "command": test_model::reference_module(),
            "exports": {"context": {"simple": {}}, "policy": {"allow_all": {}}}
        });
        if workflow != "python_agent_loop" {
            reference["exports"]["workflow"] = json!({workflow: {}});
        } else {
            config.components.insert("python-workflow".into(), serde_json::from_value(json!({
                "command": "python3", "args": ["-B", workspace_file("examples/modules/agent-worker/agent.py")],
                "exports": {"workflow": {workflow: {}}}
            })).unwrap());
        }
        config.components.insert(
            "reference".into(),
            serde_json::from_value(reference).unwrap(),
        );
        let runtime = AgentRuntime::builder(config.clone(), workspace)
            .with_config_path(Some(&config_path))
            .build_async()
            .await
            .unwrap();
        runtime.run("hello".into()).await.unwrap();
        let store = SessionStore::open(runtime.session_dir().unwrap().to_owned()).unwrap();
        let projection = store.load_projection().unwrap();
        assert!(projection.records.iter().any(|record| matches!(&record.entry,
            JournalEntry::HistoryMutated(mutation) if mutation.mutation == HistoryMutationKind::Checkpoint)));
        assert_eq!(projection.history, runtime.history().await);
        let catalog = ModuleCatalog::from_config(&config).unwrap();
        let replay = replay_workflow(
            store.session_dir(),
            &config,
            &catalog,
            WorkflowReplayOptions::default(),
        )
        .await
        .unwrap();
        assert!(
            replay.comparison.matched,
            "{workflow}: {:?}",
            replay.comparison.issues
        );
    }
}

fn preparation_probe_config() -> AppConfig {
    let mut config = test_model::config();
    config.modules.workflow = Some("checkpoint_probe".into());
    test_model::add_allow_all_policy(&mut config);
    config.components.insert("checkpoint-probe".into(),serde_json::from_value(json!({
        "command":"python3", "args":["-B",workspace_file("crates/proteus-core/tests/fixtures/preparation_checkpoint_workflow.py")],
        "exports":{"workflow":{"checkpoint_probe":{}}}
    })).unwrap());
    config.tools.enabled = vec![];
    config.tools.configured.push(
        serde_json::from_value(json!({
            "name":"checkpoint_probe", "description":"unbound effect", "safety":"ReadOnly",
            "executor":{"kind":"process", "command":"sh", "args":["-c","cat >/dev/null; printf child"]}
        }))
        .unwrap(),
    );
    config
}

#[tokio::test]
async fn preparation_checkpoint_rejects_stale_revision_and_unbound_tool_does_not_advance_it() {
    for persist in [false, true] {
        let workspace = tempfile::tempdir().unwrap();
        let config_path = workspace.path().join("config.toml");
        let runtime = AgentRuntime::builder(preparation_probe_config(), workspace.path().into())
            .with_config_path(persist.then_some(config_path.as_path()))
            .build_async()
            .await
            .unwrap();
        runtime.run("seed".into()).await.unwrap();
        assert_eq!(
            runtime.run("unbound_tool".into()).await.unwrap().text,
            "unbound tool preserved revision"
        );
        assert!(
            runtime
                .history()
                .await
                .iter()
                .all(|message| message.tool_call_id.is_none())
        );
        assert_eq!(
            runtime.run("stale".into()).await.unwrap().text,
            "stale rejected"
        );
        let error = runtime.run("stale_terminal".into()).await.unwrap_err();
        assert!(
            format!("{error:#}")
                .contains("success after rejecting a context preparation checkpoint"),
            "{error:#}"
        );
        assert!(
            runtime
                .history()
                .await
                .iter()
                .all(|message| message.display_text() != "REJECTED CANDIDATE")
        );
    }
}

#[tokio::test]
async fn lost_preparation_acknowledgement_cannot_roll_back_committed_history() {
    for persist in [false, true] {
        let workspace = tempfile::tempdir().unwrap();
        let config_path = workspace.path().join("config.toml");
        let runtime = AgentRuntime::builder(preparation_probe_config(), workspace.path().into())
            .with_config_path(persist.then_some(config_path.as_path()))
            .build_async()
            .await
            .unwrap();
        runtime.run("seed".into()).await.unwrap();
        let error = runtime.run("lost_ack".into()).await.unwrap_err();
        assert!(
            format!("{error:#}").contains("lost preparation acknowledgement"),
            "{error:#}"
        );
        let history = runtime.history().await;
        assert!(
            history
                .iter()
                .any(|message| message.display_text() == "COMMITTED PREPARATION")
        );
        assert!(
            history
                .iter()
                .all(|message| message.display_text() != "OLD ASSISTANT")
        );
        assert_eq!(history.last().unwrap().display_text(), "lost_ack");
        if let Some(dir) = runtime.session_dir() {
            assert_eq!(
                SessionStore::open(dir.to_owned())
                    .unwrap()
                    .load_messages()
                    .unwrap(),
                history
            );
        }
    }
}
