use super::*;
use proteus_core::core::{
    AgentControlSurface, AgentRuntime, AppConfig, JournalEntry, ModuleCatalog, SessionStore,
    WorkflowReplayOptions, replay_workflow,
};

/// Exercise the shipped composition with real restricted workers and trusted
/// DCP. Only the model, executable locations and personal state are fixtures.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shipped_restricted_profile_runs_search_callbacks_and_replays_a_file_turn() -> Result<()> {
    let state = tempfile::tempdir()?;
    let workspace = state.path().join("workspace");
    let home = state.path().join("home");
    fs::create_dir(&workspace)?;
    fs::create_dir(&home)?;
    fs::create_dir(workspace.join(".git"))?;
    fs::write(
        workspace.join("AGENTS.md"),
        "PROFILE_INSTRUCTIONS_SENTINEL\n",
    )?;
    fs::write(
        workspace.join("probe.txt"),
        "read_file PROFILE_SEARCH_SENTINEL\n",
    )?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut config = AppConfig::load(Some(&root.join("configs/restricted.config.toml"))).await?;
    let fake = test_model::config();
    config
        .components
        .retain(|_, component| !component.exports().any(|(slot, _, _)| slot == "model"));
    config.components.extend(fake.components);
    config.providers = fake.providers;
    config.active_provider = fake.active_provider;
    config
        .module_config
        .insert("model".into(), fake.module_config["model"].clone());
    config.agent_control.surface = AgentControlSurface::None;
    config.agent_control.roles.clear();
    for (id, component) in &mut config.components {
        let mut value = serde_json::to_value(&*component)?;
        if id == "dcp" {
            value["command"] = json!("node");
            value["args"] = json!([std::env::var("PROTEUS_TEST_DCP_MODULE")?]);
        } else {
            value["command"] = json!(test_model::reference_module());
        }
        if value.get("sandbox").is_none() {
            // Keep trusted workers away from the owner's credentials/plugins.
            value["env_allowlist"] = json!([]);
            value["env"] = json!({"HOME":home, "PROTEUS_HOME":home, "XDG_DATA_HOME":home});
        }
        *component = serde_json::from_value(value)?;
    }
    for (slot, id) in [("hook", "hook.dcp"), ("tool", "dcp.tools")] {
        let dcp = config
            .module_config
            .get_mut(slot)
            .unwrap()
            .get_mut(id)
            .unwrap();
        dcp["state_dir"] = json!(state.path().join("dcp-state"));
    }
    // Search is opt-in in the context builder. Exercise its restricted provider
    // through the normal host callback in addition to the direct search tool.
    let context = config
        .module_config
        .get_mut("context")
        .unwrap()
        .get_mut("codex_context")
        .unwrap();
    context["providers"]
        .as_array_mut()
        .unwrap()
        .push(json!("search"));
    context["search_provider"] = json!("rg_search");

    let registry = RuntimeRegistry::from_config(&config, workspace.clone())?;
    let found = registry
        .tools
        .get("search")
        .expect("search tool")
        .invoke(
            &ToolCall::new(
                new_call_id(),
                "search",
                json!({"query":"PROFILE_SEARCH_SENTINEL"}),
            ),
            ToolContext::new(
                workspace.clone(),
                ExecutionAttribution::detached(new_execution_id()),
            ),
        )
        .await?;
    assert!(
        found.ok && found.output.contains("PROFILE_SEARCH_SENTINEL"),
        "{found:?}"
    );
    drop(registry);

    let runtime = AgentRuntime::builder(config.clone(), workspace.clone())
        .with_config_path(Some(&state.path().join("profile.toml")))
        .build_async()
        .await?;
    // The fake's patch trigger accepts trailing context annotations, including
    // DCP message IDs; its literal-path read_file trigger intentionally does not.
    let output = runtime
        .run("apply_patch PROFILE_SEARCH_SENTINEL".into())
        .await?;
    assert!(output.text.contains("smoke.txt"), "{output:?}");
    assert_eq!(fs::read_to_string(workspace.join("smoke.txt"))?, "smoke\n");
    let store = SessionStore::open(runtime.session_dir().unwrap().to_owned())?;
    let projection = store.load_projection()?;
    assert_eq!(projection.history, runtime.history().await);
    assert!(projection.records.iter().any(|record| matches!(&record.entry,
        JournalEntry::ToolResultRecorded(tool) if tool.result.ok && tool.result.output.contains("smoke.txt"))));
    let contexts = projection
        .records
        .iter()
        .filter_map(|record| match &record.entry {
            JournalEntry::ContextResponseRecorded(context) => {
                Some(serde_json::to_string(context).unwrap())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        contexts.contains("PROFILE_INSTRUCTIONS_SENTINEL"),
        "{contexts}"
    );
    assert!(
        contexts.contains("codex_context:search:rg"),
        "restricted search provider callback: {contexts}"
    );
    for hook in ["codex_compact", "hook.dcp"] {
        assert!(projection.records.iter().any(|record| matches!(&record.entry,
            JournalEntry::HookInvoked(trace) if trace.steps.iter().any(|step| step.module_id == hook))), "missing hook {hook}");
    }
    let catalog = ModuleCatalog::from_config(&config)?;
    let replay = replay_workflow(
        store.session_dir(),
        &config,
        &catalog,
        WorkflowReplayOptions::default(),
    )
    .await?;
    assert!(
        replay.comparison.matched,
        "{:?}; {:?}",
        replay.comparison.issues, replay.replay.error
    );
    assert!(replay.source_journal_unchanged);
    Ok(())
}
