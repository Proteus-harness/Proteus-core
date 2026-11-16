//! Переносимые рабочие profiles и фактические facade дочерних ролей.
use super::*;
use proteus_core::core::AgentControlRuntime;

#[tokio::test]
async fn standalone_working_profiles_share_go_without_replacing_native_behavior_slots() {
    let root = workspace_root();
    for (name, workflow, context, tools) in [
        (
            "config.toml",
            "coding.codex_loop",
            "codex_context",
            vec![
                "exec_command",
                "write_stdin",
                "apply_patch",
                "update_plan",
                "request_user_input",
            ],
        ),
        (
            "codex.config.toml",
            "coding.codex_loop",
            "codex_context",
            vec![
                "exec_command",
                "write_stdin",
                "apply_patch",
                "update_plan",
                "request_user_input",
            ],
        ),
        (
            "pi.config.toml",
            "pi_loop",
            "pi_context",
            vec!["read", "bash", "edit", "write"],
        ),
        (
            "opencode.config.toml",
            "opencode_loop",
            "opencode_context",
            vec!["read", "write", "edit", "patch", "shell", "glob", "grep"],
        ),
    ] {
        let moved = tempfile::tempdir().unwrap();
        let path = moved.path().join(name);
        std::fs::copy(root.join("configs").join(name), &path).unwrap();
        let config = AppConfig::load(Some(&path)).await.unwrap();
        assert_eq!(config.active_provider.as_deref(), Some("cheap"));
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.providers["cheap"].provider, "opencode");
        assert_eq!(config.providers["cheap"].model, "deepseek-v4.1-flash");
        assert_eq!(
            config.providers["cheap"].reasoning.effort.as_deref(),
            Some("high")
        );
        assert_eq!(
            config.process_export_config("model", "opencode").unwrap()["provider"],
            "opencode-go"
        );
        assert_eq!(config.modules.workflow.as_deref(), Some(workflow));
        assert_eq!(config.modules.context.as_deref(), Some(context));
        assert_eq!(config.tools.enabled, tools);
        let catalog = ModuleCatalog::from_config(&config).unwrap();
        assert!(
            catalog
                .manifest(proteus_contracts::domain::ModuleKind::Model, "opencode")
                .is_some()
        );
        if name == "pi.config.toml" {
            assert_eq!(
                config.process_export_config("workflow", "pi_loop").unwrap()["catalog_provider"],
                "opencode-go"
            );
            assert_eq!(
                config.process_export_config("hook", "pi_compact").unwrap()["catalog_provider"],
                "opencode-go"
            );
        }
        // Alias and effort selection must not pick another workflow/tool catalog.
        let changed = std::fs::read_to_string(&path)
            .unwrap()
            .replace("\"cheap\"", "\"economy\"")
            .replace("models.cheap", "models.economy")
            .replace("effort = \"high\"", "effort = \"max\"");
        std::fs::write(&path, changed).unwrap();
        let rebound = AppConfig::load(Some(&path)).await.unwrap();
        assert_eq!(
            rebound.providers["economy"].reasoning.effort.as_deref(),
            Some("max")
        );
        assert_eq!(
            behavior_without_model_or_labels(&config),
            behavior_without_model_or_labels(&rebound)
        );
        assert_eq!(std::fs::read_dir(moved.path()).unwrap().count(), 1);
    }
}

#[tokio::test]
async fn collaboration_and_worktree_task_profiles_advertise_only_supported_roles() {
    use proteus_contracts::contracts::{AgentIsolation, ToolRegistry};
    let root = workspace_root();
    let collaboration = AppConfig::load(Some(&root.join("configs/agent.toml")))
        .await
        .unwrap();
    let mut tools = ToolRegistry::new();
    AgentControlRuntime::from_config(&collaboration.agent_control)
        .unwrap()
        .register_tools(&mut tools, 1000)
        .unwrap();
    assert_eq!(collaboration.agent_control.roles.len(), 1);
    assert_eq!(
        tools.get("spawn_agent").unwrap().spec().input_schema["properties"]["agent_type"]["enum"],
        serde_json::json!(["explore"])
    );
    assert!(tools.get("task").is_none());

    let moved = tempfile::tempdir().unwrap();
    let path = moved.path().join("codex-task.toml");
    std::fs::copy(root.join("configs/codex-task.config.toml"), &path).unwrap();
    let task = AppConfig::load(Some(&path)).await.unwrap();
    let runtime = AgentControlRuntime::from_config(&task.agent_control).unwrap();
    let profiles = runtime.service().unwrap().profiles();
    let coder = profiles.iter().find(|role| role.name == "coder").unwrap();
    assert_eq!(coder.isolation, AgentIsolation::Worktree);
    assert!(!coder.parallel_safe);
    let mut tools = ToolRegistry::new();
    runtime.register_tools(&mut tools, 1000).unwrap();
    assert!(tools.get("task").is_some());
    assert!(tools.get("spawn_agent").is_none());
    for role in &task.agent_control.roles {
        assert!(role.model.is_none());
        let child = AppConfig::load(Some(Path::new(&role.config)))
            .await
            .unwrap();
        assert_eq!(child.modules.hooks, task.modules.hooks);
        assert!(child.agent_control.roles.is_empty());
        if role.name == "coder" {
            assert_eq!(child.tools.enabled, task.tools.enabled);
            assert_eq!(
                child
                    .process_export_config("policy", "codex_policy")
                    .unwrap(),
                task.process_export_config("policy", "codex_policy")
                    .unwrap()
            );
        }
    }
}

#[tokio::test]
async fn standalone_agent_moves_as_one_file_and_preserves_root_and_peer_behaviors() {
    let root = workspace_root();
    let moved = tempfile::tempdir().unwrap();
    let path = moved.path().join("agent.toml");
    std::fs::copy(root.join("configs/agent.toml"), &path).unwrap();
    let config = AppConfig::load(Some(&path)).await.unwrap();
    let baseline = AppConfig::load(Some(&root.join("configs/codex.config.toml")))
        .await
        .unwrap();
    assert_eq!(
        behavior_without_model_or_labels(&config),
        behavior_without_model_or_labels(&baseline)
    );
    assert_eq!(config.profile.name, AppConfig::default().profile.name);
    assert_eq!(config.providers.len(), 1);
    assert_eq!(config.providers["cheap"].provider, "opencode");
    assert_eq!(config.providers["cheap"].model, "deepseek-v4.1-flash");
    assert_eq!(
        config.providers["cheap"].reasoning.effort.as_deref(),
        Some("high")
    );
    assert_eq!(config.agent_control.roles.len(), 1);
    for role in &config.agent_control.roles {
        let reference = Path::new(&role.config);
        assert_eq!(proteus_core::core::physical_config_path(reference), path);
        let child = AppConfig::load(Some(reference)).await.unwrap();
        let baseline = AppConfig::load(Some(
            &root.join(format!("configs/codex-{}.config.toml", role.name)),
        ))
        .await
        .unwrap();
        assert_eq!(
            behavior_without_model_or_labels(&child),
            behavior_without_model_or_labels(&baseline),
            "{} behavior",
            role.name
        );
        assert_eq!(child.providers["cheap"].model, "deepseek-v4.1-flash");
        assert!(
            role.model.is_none(),
            "the role must inherit the parent selection"
        );
        assert_eq!(
            child.modules.hooks,
            ["codex_compact", "codex_base_instructions"]
        );
        assert!(
            child
                .instructions
                .iter()
                .all(|instruction| instruction.priority == 110)
        );
        assert!(
            child
                .instructions
                .iter()
                .all(|instruction| instruction.file.is_none() && instruction.text.is_some())
        );
    }
    assert_eq!(std::fs::read_dir(moved.path()).unwrap().count(), 1);
}
