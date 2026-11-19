//! The shipped experimental profile uses the same execution-bound policy path as tools.
#[path = "support/model.rs"]
mod test_model;

use proteus_contracts::{
    contracts::{CancellationToken, ExecutionScope, PolicyContext, PolicyVisibilityContext},
    domain::{PermissionMode, PolicyDecision, ToolCall, ToolSafety, ToolSpec},
};
use proteus_core::core::{
    AppConfig, BoundTools, HeadlessApprovalTransport, ModelExecutionBinding, ModuleEpoch,
    PreparedAssembly, RuntimeSnapshot, ToolExecutionBinding,
};
use serde_json::json;
use std::{path::Path, sync::Arc};

#[tokio::test]
async fn experimental_profile_confirms_writes_and_commands_and_full_access_keeps_hard_denials() {
    let profile = AppConfig::load(Some(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs/config.toml"),
    ))
    .await
    .unwrap();
    let mut config = test_model::config();
    config.modules.policy = profile.modules.policy.clone();
    config.tools.enabled = vec!["request_permissions".into()];
    config
        .module_config
        .insert("policy".into(), profile.module_config["policy"].clone());
    config
        .module_config
        .get_mut("policy")
        .unwrap()
        .get_mut("codex_policy")
        .unwrap()["deny"] = json!(["blocked"]);
    config.components.insert(
        "test-policy".into(),
        serde_json::from_value(json!({
            "command": test_model::reference_module(), "exports": {"policy": {"codex_policy": {}}, "tool": {"policy_tools": {}}}
        }))
        .unwrap(),
    );
    let workspace = tempfile::tempdir().unwrap();
    let assembly = PreparedAssembly::from_config(config, workspace.path().into(), None).unwrap();
    let snapshot = RuntimeSnapshot::new(ModuleEpoch::initial(), assembly, None);
    for mode in PermissionMode::SELECTABLE
        .into_iter()
        .chain([PermissionMode::Auto])
    {
        let execution = snapshot
            .registry
            .execution_context(
                ModelExecutionBinding::detached(ExecutionScope::fresh(CancellationToken::new())),
                Arc::new(HeadlessApprovalTransport),
                mode,
            )
            .unwrap();
        assert!(execution.permission_grants.snapshot().is_empty());
        if mode == PermissionMode::FullAccess {
            let tools = BoundTools::new(
                execution.tools.clone(),
                execution.policy.clone(),
                execution.approval.clone(),
                execution.permission_grants.clone(),
                ToolExecutionBinding::detached(execution.scope.clone()),
            );
            let result = tools
                .execute(
                    workspace.path().into(),
                    ToolCall::new(
                        "grant-call",
                        "request_permissions",
                        json!({"permissions":["escalated_exec"], "justification":"fixture"}),
                    ),
                )
                .await
                .unwrap();
            assert!(result.ok, "{result:?}");
            assert_eq!(
                result.metadata["granted_permissions"],
                json!(["escalated_exec"])
            );
            // Only actual user approval can publish these metadata grants.
            assert!(execution.permission_grants.snapshot().is_empty());
        }
        for (name, safety) in [
            ("read_file", ToolSafety::ReadOnly),
            ("apply_patch", ToolSafety::WritesFiles),
            ("write_file", ToolSafety::WritesFiles),
            ("shell", ToolSafety::RunsCommands),
            ("exec_command", ToolSafety::RunsCommands),
            ("blocked", ToolSafety::ReadOnly),
        ] {
            let spec = ToolSpec::new(name, "fixture", json!({"type":"object"}), safety.clone());
            let decision = execution.policy.evaluate(
                &ToolCall::new("call-1", name, json!({})),
                &PolicyContext::new(workspace.path().into(), Some(spec.clone())),
            );
            let visible = execution
                .policy
                .evaluate_visibility(&PolicyVisibilityContext::new(workspace.path().into(), spec));
            if name == "blocked"
                || (mode == PermissionMode::Plan && safety != ToolSafety::ReadOnly)
                || (mode == PermissionMode::Auto && safety == ToolSafety::RunsCommands)
            {
                assert!(
                    matches!(decision, PolicyDecision::Deny { .. }),
                    "{mode:?}/{name}: {decision:?}"
                );
                assert!(matches!(visible, PolicyDecision::Deny { .. }));
            } else if mode == PermissionMode::Normal && safety != ToolSafety::ReadOnly {
                assert!(
                    matches!(decision, PolicyDecision::Ask { .. }),
                    "{name}: {decision:?}"
                );
                assert!(matches!(visible, PolicyDecision::Ask { .. }));
            } else {
                assert_eq!(decision, PolicyDecision::Allow, "{mode:?}/{name}");
                assert_eq!(visible, PolicyDecision::Allow);
            }
        }
    }
}
