use super::*;

#[tokio::test]
async fn independently_linked_packages_preserve_core_tool_registration_and_results() {
    let binaries: std::collections::BTreeMap<String, String> = serde_json::from_str(
        &std::env::var("PROTEUS_TEST_MODULE_BINARIES")
            .expect("run scripts/test.py for package fixtures"),
    )
    .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        workspace.path().join("sample.rs"),
        "needle in an independent process\n",
    )
    .unwrap();
    let aggregate = test_model::reference_module();
    let mut results = vec![];
    for independent in [false, true] {
        let mut config = test_model::config();
        config.tools.enabled = vec!["search".into(), "update_plan".into()];
        for (package, exports) in [
            (
                "rg-search",
                json!({"tool":{"rg_search":{}},"context_provider":{"rg_search":{}}}),
            ),
            ("plan-tool", json!({"tool":{"plan_tool":{}}})),
        ] {
            let command = if independent {
                PathBuf::from(&binaries[package])
            } else {
                aggregate.clone()
            };
            config.components.insert(
                package.into(),
                serde_json::from_value(json!({
                    "command":command,"exports":exports,
                }))
                .unwrap(),
            );
        }
        let selected = registry(&config, workspace.path()).unwrap();
        let searched = search(&selected, workspace.path()).await.unwrap();
        assert!(searched.ok, "{}", searched.output);
        assert!(searched.output.contains("sample.rs"));
        let planned = selected
            .tools
            .get("update_plan")
            .unwrap()
            .invoke(
                &ToolCall::new(
                    new_call_id(),
                    "update_plan",
                    json!({"plan":[
                        {"step":"Independent package","status":"in_progress"}
                    ]}),
                ),
                ToolContext::new(
                    workspace.path().into(),
                    ExecutionAttribution::detached(new_execution_id()),
                ),
            )
            .await
            .unwrap();
        assert!(planned.ok);
        results.push((
            searched.output,
            searched.metadata,
            planned.output,
            planned.metadata,
        ));
        config.tools.enabled.clear();
        let disabled = registry(&config, workspace.path()).unwrap();
        assert!(disabled.tools.get("search").is_none());
        assert!(disabled.tools.get("update_plan").is_none());
    }
    assert_eq!(results[0], results[1]);
}
