//! Exact model inheritance crosses the real child process and canonical journal.
use super::*;
use proteus_contracts::{
    app_protocol::AppSubagentSession,
    domain::{ModelRef, ReasoningConfig},
    model_standard::{
        CanonicalMessage, CanonicalModelResponse, FinishReason, MessageRole, ModelCapabilities,
    },
};
use proteus_core::core::{AppConfig, JournalEntry, SessionStore};
use serde_json::Value;
use std::path::Path;

fn reasoning(effort: &str, summary: bool, budget_tokens: Option<u32>) -> ReasoningConfig {
    let mut value = ReasoningConfig::default();
    value.effort = Some(effort.into());
    value.summary = summary;
    value.budget_tokens = budget_tokens;
    value
}

fn model_settings(provider: &str, names: &[&str]) -> Value {
    let mut capabilities = ModelCapabilities::basic_text_and_tools().with_streaming(true);
    capabilities.supports_reasoning_config = true;
    let response = CanonicalModelResponse::new(
        CanonicalMessage::text(MessageRole::Assistant, format!("served by {provider}")),
        vec![],
        FinishReason::Stop,
    );
    json!({
        "descriptor": {"adapter_id": provider, "capabilities": capabilities, "hosted_tools": []},
        "catalog": {"models": names.iter().map(|id| json!({
            "id": id, "display_name": id, "description": null, "hidden": false,
            "reasoning_efforts": ["low", "high"], "default_reasoning_effort": "low"
        })).collect::<Vec<_>>()},
        "fresh_response_ids": true,
        "events": [], "terminal": {"kind": "response", "response": response}
    })
}

async fn runner(home: &Path) -> Arc<dyn AgentControl> {
    let child_path = write_tool_surface_child_config(home, "template", &[]);
    let source: toml::Value =
        toml::from_str(&std::fs::read_to_string(&child_path).unwrap()).unwrap();
    let mut child = serde_json::to_value(source).unwrap();
    let child_object = child.as_object_mut().unwrap();
    child_object.remove("active_provider");
    child_object.remove("providers");
    child_object["components"]
        .as_object_mut()
        .unwrap()
        .remove("test-model");
    child_object["module_config"]
        .as_object_mut()
        .unwrap()
        .remove("model");
    let mut fixed = child.clone();
    fixed["model"] = "role-fixed".into();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let worker = root.join("tests/fixtures/process_model.py");
    // The model worker records its direct Core parent. Rebinding replaces
    // model workers, while fresh tasks must reuse the same Core peer.
    let probe = "import os,runpy,sys; from pathlib import Path; marker,worker=sys.argv[1:]; f=Path(marker).open('a'); f.write(str(os.getppid())+'\\n'); f.close(); sys.argv=[worker]; runpy.run_path(worker,run_name='__main__')";
    let model_args = json!(["-B", "-c", probe, home.join("core-pids.txt"), worker]);
    let config = json!({
        "model": "main",
        "models": {
            "main": {"provider": "model_a", "model": "baseline", "effort": "low", "summary": true},
            "discount": {"provider": "model_b", "model": "discount-model", "effort": "low", "summary": false, "stream": false},
            "role-fixed": {"provider": "model_a", "model": "fixed-model", "effort": "low", "summary": true}
        },
        "components": {
            "route-a": {"command": "python3", "args": model_args, "exports": {"model": {"model_a": {}}}},
            "route-b": {"command": "python3", "args": model_args, "exports": {"model": {"model_b": {}}}}
        },
        "module_config": {"model": {
            "model_a": model_settings("model_a", &["baseline", "parent-now", "parent-next", "parent-later", "fixed-model"]),
            "model_b": model_settings("model_b", &["discount-model"])
        }},
        "agent_control": {"binary": proteus_binary(), "max_idle_processes": 2, "roles": [
            {"name": "inherit", "description": "Parent model", "timeout_ms": 60000, "config": child},
            {"name": "fixed", "description": "Own named model", "timeout_ms": 60000, "config": fixed}
        ]}
    });
    let path = home.join("configs/agent.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    std::fs::remove_file(child_path).unwrap();
    let parsed = AppConfig::load(Some(&path)).await.unwrap();
    assert!(parsed.agent_control.roles[0].model.is_none());
    assert!(parsed.agent_control.roles[1].model.is_some());
    AgentControlRuntime::from_config(&parsed.agent_control)
        .unwrap()
        .service()
        .unwrap()
}

fn requests(
    result: &proteus_contracts::contracts::AgentControlResult,
) -> Vec<proteus_contracts::model_standard::CanonicalModelRequest> {
    let session: AppSubagentSession =
        serde_json::from_value(result.metadata["subagent_session"].clone()).unwrap();
    SessionStore::open(session.session_dir)
        .unwrap()
        .load_records()
        .unwrap()
        .into_iter()
        .filter_map(|record| match record.entry {
            JournalEntry::ModelRequestRecorded(request) => Some(request.request),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn fresh_children_inherit_admitted_model_reuse_resets_and_resume_stays_pinned() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let runner = runner(home.path()).await;
    let mut ctx = test_runtime_context(Arc::new(InMemoryEventStore::new()));
    ctx.model_ref = Some(ModelRef::new("model_a", "parent-now"));
    ctx.reasoning = reasoning("high", true, Some(333));
    let task = AgentTask::new("report", workspace.path().to_path_buf());
    let first = runner
        .run(
            AgentControlRequest::new("inherit", "first", task.clone()),
            ctx.clone(),
        )
        .await
        .unwrap();
    let recorded = requests(&first);
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].model, ctx.model_ref.clone().unwrap());
    assert_eq!(recorded[0].reasoning, ctx.reasoning);

    ctx.model_ref = Some(ModelRef::new("model_a", "parent-next"));
    ctx.reasoning = reasoning("low", false, Some(444));
    let second = runner
        .run(
            AgentControlRequest::new("inherit", "fresh next", task.clone()),
            ctx.clone(),
        )
        .await
        .unwrap();
    let recorded = requests(&second);
    assert_eq!(
        recorded.len(),
        1,
        "a fresh pooled lease has a distinct journal"
    );
    assert_eq!(recorded[0].model, ctx.model_ref.clone().unwrap());
    assert_eq!(recorded[0].reasoning, ctx.reasoning);
    let pinned = recorded[0].clone();
    let id = second.child_thread_id.unwrap().to_string();
    let rejected = runner
        .run(
            AgentControlRequest::new("inherit", "cannot change resume", task.clone())
                .with_metadata(json!({"task_id": id, "model": "discount"})),
            ctx.clone(),
        )
        .await
        .unwrap_err();
    assert!(rejected.to_string().contains("cannot override"));
    ctx.model_ref = Some(ModelRef::new("model_a", "parent-later"));
    ctx.reasoning.effort = Some("high".into());
    let resumed = runner
        .run(
            AgentControlRequest::new("inherit", "continue", task)
                .with_metadata(json!({"task_id": id})),
            ctx,
        )
        .await
        .unwrap();
    let recorded = requests(&resumed);
    assert_eq!(recorded.len(), 2);
    assert!(
        recorded
            .iter()
            .all(|request| request.model == pinned.model && request.reasoning == pinned.reasoning)
    );
    let parents = std::fs::read_to_string(home.path().join("core-pids.txt"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        parents.len(),
        1,
        "fresh model changes must reuse the same Core peer"
    );
    assert!(
        !home.path().join("model-selections").exists(),
        "parent choices must not become shared last-selection defaults"
    );
}

#[tokio::test]
async fn declared_role_alias_and_call_override_keep_precedence_and_rebind_real_adapter() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let runner = runner(home.path()).await;
    let mut ctx = test_runtime_context(Arc::new(InMemoryEventStore::new()));
    ctx.model_ref = Some(ModelRef::new("model_a", "parent-now"));
    ctx.reasoning.effort = Some("high".into());
    let task = AgentTask::new("report", workspace.path().to_path_buf());
    let fixed = runner
        .run(
            AgentControlRequest::new("fixed", "role model", task.clone()),
            ctx.clone(),
        )
        .await
        .unwrap();
    let recorded = requests(&fixed);
    assert_eq!(recorded[0].model.model, "fixed-model");
    assert_eq!(recorded[0].reasoning.effort.as_deref(), Some("low"));

    let selected = runner
        .run(
            AgentControlRequest::new("fixed", "call alias", task.clone())
                .with_metadata(json!({"model": "discount", "effort": "high"})),
            ctx.clone(),
        )
        .await
        .unwrap();
    assert!(
        selected.summary.contains("served by model_b"),
        "{}",
        selected.summary
    );
    let recorded = requests(&selected);
    assert_eq!(
        recorded[0].model,
        ModelRef::new("model_b", "discount-model")
    );
    assert_eq!(recorded[0].reasoning.effort.as_deref(), Some("high"));
    assert!(!recorded[0].reasoning.summary);

    let fixed_again = runner
        .run(
            AgentControlRequest::new("fixed", "fresh role again", task.clone()),
            ctx.clone(),
        )
        .await
        .unwrap();
    assert!(fixed_again.summary.contains("served by model_a"));
    assert_eq!(requests(&fixed_again)[0].model.model, "fixed-model");
    let before =
        proteus_core::core::list_workspace_session_summaries(home.path(), workspace.path())
            .unwrap();
    let invalid = runner
        .run(
            AgentControlRequest::new("fixed", "must never reach generation", task)
                .with_metadata(json!({"model": "discount", "effort": "ultra"})),
            ctx,
        )
        .await
        .unwrap_err();
    assert!(
        invalid.to_string().contains("model selection failed"),
        "{invalid:#}"
    );
    let after = proteus_core::core::list_workspace_session_summaries(home.path(), workspace.path())
        .unwrap();
    for session in after
        .iter()
        .filter(|row| !before.iter().any(|old| old.session_id == row.session_id))
    {
        let records = SessionStore::open(session.session_dir.clone())
            .unwrap()
            .load_records()
            .unwrap();
        assert!(
            records
                .iter()
                .all(|record| !matches!(record.entry, JournalEntry::ModelRequestRecorded(_)))
        );
    }
    assert!(!home.path().join("model-selections").exists());
}
