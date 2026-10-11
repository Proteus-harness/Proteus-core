use super::*;
use proteus_contracts::{
    contracts::{
        CancellationToken, ExecutionHooks, ExecutionScope, NoopExecutionRecorder,
        ToolSelectionInput, ToolSelectionOutput, ToolSelectionRequest,
    },
    domain::{ToolSafety, ToolSpec},
};

#[tokio::test]
async fn external_selector_can_only_return_unchanged_unique_candidates() {
    for mode in [
        "valid",
        "hosted",
        "schema",
        "parallel",
        "invented",
        "duplicate",
        "unknown",
    ] {
        let workspace = tempfile::tempdir().unwrap();
        let mut config = test_model::config();
        config.modules.hooks = vec!["probe.selection".into()];
        config.components.insert("selection".into(), serde_json::from_value(json!({
            "command":"python3", "args":["-B",workspace_file("crates/proteus-core/tests/fixtures/process_tool_selection.py")],
            "exports":{"hook":{"probe.selection":{}}}
        })).unwrap());
        config
            .module_config
            .entry("hook".into())
            .or_default()
            .insert("probe.selection".into(), json!({"mode":mode}));
        let registry = registry(&config, workspace.path()).unwrap();
        let tool = ToolSpec::new(
            "web_search",
            "local read",
            json!({"type":"object"}),
            ToolSafety::ReadOnly,
        );
        let scope = ExecutionScope::fresh(CancellationToken::new());
        let chain = proteus_core::core::RuntimeHookChain::new(
            registry.hooks.clone(),
            scope.clone(),
            Arc::new(NoopExecutionRecorder),
            None,
        );
        let selected = chain
            .apply(HookInput {
                event: HookEvent::BeforeToolSelection {
                    input: ToolSelectionInput::new(
                        ToolSelectionRequest::new(AgentTask::new("probe", workspace.path().into())),
                        vec![tool.clone()],
                    ),
                    output: ToolSelectionOutput::new(vec![tool.clone()]),
                },
                attribution: ExecutionAttribution::detached(scope.execution_id),
                cwd: workspace.path().into(),
                conversation: None,
            })
            .await;
        if mode == "valid" {
            let HookEvent::BeforeToolSelection { output, .. } = selected.unwrap() else {
                panic!("unexpected event")
            };
            assert_eq!(output.tools, vec![tool]);
        } else {
            assert!(selected.is_err(), "accepted {mode}");
        }
    }
}

#[tokio::test]
async fn dynamic_selection_is_replaceable_and_ordered_hooks_only_narrow() {
    for (order, mode, expected) in [
        (
            vec!["tool_selection.dynamic", "probe.narrow"],
            "last",
            Some("search"),
        ),
        (
            vec!["probe.narrow", "tool_selection.dynamic"],
            "last",
            Some("read_file"),
        ),
        (
            vec!["tool_selection.dynamic", "probe.narrow"],
            "reexpand",
            None,
        ),
    ] {
        let workspace = tempfile::tempdir().unwrap();
        let mut config = test_model::config();
        config.modules.hooks = order.iter().map(|id| (*id).into()).collect();
        config.components.insert("dynamic".into(), serde_json::from_value(json!({
            "command":test_model::reference_module(), "exports":{"hook":{"tool_selection.dynamic":{}}}
        })).unwrap());
        config.components.insert("narrow".into(), serde_json::from_value(json!({
            "command":"python3", "args":["-B",workspace_file("crates/proteus-core/tests/fixtures/process_tool_selection.py")],
            "exports":{"hook":{"probe.narrow":{}}}
        })).unwrap());
        config
            .module_config
            .entry("hook".into())
            .or_default()
            .insert("probe.narrow".into(), json!({"mode":mode}));
        let selected_registry = registry(&config, workspace.path()).unwrap();
        let scope = ExecutionScope::fresh(CancellationToken::new());
        let chain = proteus_core::core::RuntimeHookChain::new(
            selected_registry.hooks.clone(),
            scope.clone(),
            Arc::new(NoopExecutionRecorder),
            None,
        );
        let candidates: Vec<_> = ["shell", "search", "read_file"]
            .into_iter()
            .map(|name| ToolSpec::new(name, name, json!({"type":"object"}), ToolSafety::ReadOnly))
            .collect();
        let selected = chain
            .apply(HookInput {
                event: HookEvent::BeforeToolSelection {
                    input: ToolSelectionInput::new(
                        ToolSelectionRequest::new(AgentTask::new("probe", workspace.path().into()))
                            .with_max_tools(2)
                            .with_phase("review"),
                        candidates.clone(),
                    ),
                    output: ToolSelectionOutput::new(candidates),
                },
                attribution: ExecutionAttribution::detached(scope.execution_id),
                cwd: workspace.path().into(),
                conversation: None,
            })
            .await;
        if let Some(expected) = expected {
            let HookEvent::BeforeToolSelection { output, .. } = selected.unwrap() else {
                panic!("unexpected event")
            };
            assert_eq!(
                output
                    .tools
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>(),
                vec![expected]
            );
        } else {
            assert!(selected.is_err(), "later hook reintroduced removed tool");
        }
    }
}
