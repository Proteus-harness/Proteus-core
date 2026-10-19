use super::*;
use proteus_contracts::{
    contracts::{Tool, ToolContext, ToolRegistry},
    domain::{ToolCall, ToolResult, ToolSpec},
};

#[test]
fn doctor_flags_unknown_tool_names_in_module_config_lists() {
    struct StaticTool(&'static str);

    #[async_trait::async_trait]
    impl Tool for StaticTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec::new(
                self.0,
                "static test tool",
                serde_json::json!({ "type": "object" }),
                ToolSafety::ReadOnly,
            )
        }

        async fn invoke(&self, _call: &ToolCall, _ctx: ToolContext) -> anyhow::Result<ToolResult> {
            unreachable!("doctor check never invokes tools")
        }
    }

    let mut registry = ToolRegistry::new();
    registry
        .register(StaticTool("read_file"))
        .expect("register read_file");

    let mut config = crate::test_model::config();
    config.tools.mcp_servers.push(
        serde_json::from_value(serde_json::json!({
            "name": "playwright",
            "command": "npx",
        }))
        .expect("mcp server config"),
    );
    config.module_config.insert(
        "policy".to_owned(),
        std::collections::BTreeMap::from([(
            "codex_policy".to_owned(),
            serde_json::json!({
                "allow": ["read_file", "misspelled_tool"],
                "ask_before": ["playwright__browser_click", "ghost__tool"],
            }),
        )]),
    );

    let mut findings = DoctorFindings::default();
    check_module_config_tool_references(&mut findings, &config, &registry);

    assert!(!findings.has_errors());
    let warnings = findings
        .entries
        .iter()
        .filter(|entry| entry.level == "warn")
        .map(|entry| entry.message.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        warnings,
        vec![
            "module_config.policy.codex_policy.allow references unknown tool 'misspelled_tool'",
            "module_config.policy.codex_policy.ask_before references unknown tool 'ghost__tool'",
        ]
    );
}

#[test]
fn doctor_counts_resolved_module_config_tool_references() {
    let registry = ToolRegistry::new();
    let mut config = crate::test_model::config();
    config.tools.mcp_servers.push(
        serde_json::from_value(serde_json::json!({
            "name": "playwright",
            "command": "npx",
        }))
        .expect("mcp server config"),
    );
    config.module_config.insert(
        "hook".to_owned(),
        std::collections::BTreeMap::from([(
            "tool_selection.dynamic".to_owned(),
            serde_json::json!({ "always_include": ["playwright__browser_navigate"] }),
        )]),
    );

    let mut findings = DoctorFindings::default();
    check_module_config_tool_references(&mut findings, &config, &registry);

    assert!(!findings.has_errors());
    assert!(
        findings
            .entries
            .iter()
            .any(|entry| entry.message == "module_config tool references: 1 resolved")
    );
}

#[test]
fn doctor_checks_nested_module_config_tool_lists() {
    let registry = ToolRegistry::new();
    let mut config = crate::test_model::config();
    config.module_config.insert(
        "policy".to_owned(),
        std::collections::BTreeMap::from([(
            "opencode_policy".to_owned(),
            serde_json::json!({
                "groups": {
                    "edit": { "tools": ["missing_edit_tool"], "pattern_args": ["path"] },
                },
            }),
        )]),
    );

    let mut findings = DoctorFindings::default();
    check_module_config_tool_references(&mut findings, &config, &registry);

    assert!(findings.entries.iter().any(|entry| entry.level == "warn"
        && entry.message
            == "module_config.policy.opencode_policy.groups.edit.tools references unknown tool 'missing_edit_tool'"));
}

#[test]
fn doctor_accepts_fake_model_without_secret() {
    let config = crate::test_model::config();
    let mut findings = DoctorFindings::default();

    check_model_config(&mut findings, &config);

    assert!(!findings.has_errors());
    assert!(findings.entries.iter().any(|entry| entry.message
        == "model credentials and endpoint are owned by the selected component"));
}

#[test]
fn doctor_resolves_relative_commands_from_cwd() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("tool.sh"), "#!/bin/sh\n").expect("tool");

    assert!(command_resolves("./tool.sh", dir.path()));
    assert!(!command_resolves("./missing.sh", dir.path()));
}

#[test]
fn doctor_warns_on_short_model_timeout() {
    let mut findings = DoctorFindings::default();

    check_timeout_ms(&mut findings, "runtime.model_timeout_ms", 1_000, 120_000);

    assert!(
        findings
            .entries
            .iter()
            .any(|entry| entry.level == "warn" && entry.message.contains("too low"))
    );
}

#[test]
fn doctor_formats_timeouts_for_readability() {
    assert_eq!(format_timeout_ms(0), "disabled");
    assert_eq!(format_timeout_ms(120_000), "2m");
    assert_eq!(format_timeout_ms(10_800_000), "3h");
    assert_eq!(format_timeout_ms(1_500), "1500ms");
}
