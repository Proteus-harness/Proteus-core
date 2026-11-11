use super::*;
use proteus_contracts::contracts::{CancellationToken, OperationOwner, ProcessModelDescriptor};
use proteus_core::core::{
    JournalEntry, SessionStore, ToolCallRecordPhase, TurnSettlementStatus, WorkflowReplayOptions,
    replay_workflow,
};

const RAW_MARKER: &str = "PRIVATE_SYNTHETIC_RAW_RESULT";
const SUMMARY: &str = "SUMMARY:2:1";
const SCRIPT: &str = r#"
const reads = await Promise.all(['one.txt', 'two.txt'].map(path => tools.call('read_file', {path})));
if (reads.some(result => !result.ok)) throw Error('read failed');
const denied = await tools.call('write_file', {path: 'denied.txt', content: 'must not be written'});
const patch = await tools.call('apply_patch', {patch: '*** Begin Patch\n*** Add File: patched.txt\n+approved script write\n*** End Patch'});
if (!patch.ok) throw Error(patch.error);
text('SUMMARY:' + reads.length + ':' + Number(!denied.ok));
"#;

fn config(code: &str, independent: bool) -> AppConfig {
    let binaries: std::collections::BTreeMap<String, PathBuf> = serde_json::from_str(
        &std::env::var("PROTEUS_TEST_MODULE_BINARIES").expect("run scripts/test.py"),
    )
    .unwrap();
    let mut config = test_model::config();
    config.modules.workflow = Some("coding.single_loop".into());
    config.modules.context = Some("simple".into());
    config.modules.policy = Some("codex_policy".into());
    config.agent_control.surface = proteus_core::core::AgentControlSurface::None;
    config.tools.enabled = vec![
        "code_mode".into(),
        "read_file".into(),
        "write_file".into(),
        "apply_patch".into(),
    ];
    config.runtime.workflow_timeout_ms = 10_000;
    config.components.insert(
        "reference".into(),
        serde_json::from_value(json!({
            "command": test_model::reference_module(),
            "exports": {"workflow": {"coding.single_loop": {}}, "context": {"simple": {}},
                        "policy": {"codex_policy": {}}, "tool": {"file_tools": {}, "codex_patch": {}}}
        }))
        .unwrap(),
    );
    config.components.insert("code-mode".into(), serde_json::from_value(json!({
        "command": if independent { binaries["code-mode"].clone() } else { test_model::reference_module() },
        "exports": {"tool": {"code_mode": {}}}
    })).unwrap());
    config.components.insert(
        "test-model".into(),
        serde_json::from_value(json!({
            "command": "python3",
            "args": ["-B", workspace_file("crates/proteus-core/tests/fixtures/code_mode_model.py")],
            "exports": {"model": {"fake": {}}}
        }))
        .unwrap(),
    );
    config.module_config.get_mut("model").unwrap().insert("fake".into(), json!({
        "descriptor": ProcessModelDescriptor {
            adapter_id: "code-mode-test".into(),
            capabilities: proteus_contracts::model_standard::ModelCapabilities::basic_text_and_tools(),
            hosted_tools: vec![],
        },
        "code": code
    }));
    config
        .module_config
        .entry("policy".into())
        .or_default()
        .insert(
            "codex_policy".into(),
            json!({"deny": ["write_file"], "allow": ["apply_patch"]}),
        );
    config
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scripts_preserve_policy_nested_ownership_compact_context_and_replay_in_both_packages() {
    for (independent, script_error, root_error) in [
        (false, false, false),
        (true, false, false),
        (true, true, true),
    ] {
        let root = tempfile::tempdir().unwrap();
        for path in ["one.txt", "two.txt"] {
            std::fs::write(root.path().join(path), format!("{RAW_MARKER} {path}\n")).unwrap();
        }
        let code = if script_error {
            format!("{SCRIPT}\nthrow Error('synthetic script failure');")
        } else {
            SCRIPT.into()
        };
        let mut config = config(&code, independent);
        if root_error {
            config
                .module_config
                .get_mut("model")
                .unwrap()
                .get_mut("fake")
                .unwrap()["fail_after_result"] = json!(true);
        }
        let runtime = AgentRuntime::builder(config.clone(), root.path().into())
            .with_config_path(Some(&root.path().join("config.json")))
            .build_async()
            .await
            .unwrap();
        let output = runtime.run("Run the synthetic script".into()).await;
        assert_eq!(output.is_err(), root_error, "{output:?}");
        if let Ok(output) = output {
            assert_eq!(output.text.trim(), SUMMARY);
        }
        assert!(!root.path().join("denied.txt").exists());
        assert_eq!(
            std::fs::read_to_string(root.path().join("patched.txt")).unwrap(),
            "approved script write\n"
        );
        let session = runtime.session_dir().unwrap().to_owned();
        let projection = SessionStore::open(session.clone())
            .unwrap()
            .load_projection()
            .unwrap();
        let requested: Vec<_> = projection
            .records
            .iter()
            .filter_map(|record| match &record.entry {
                JournalEntry::ToolCallRecorded(call) => match &call.phase {
                    ToolCallRecordPhase::Requested { owner } => Some((&call.call, owner)),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(requested.len(), 5);
        let parent = requested
            .iter()
            .find(|(call, _)| call.name == "code_mode")
            .unwrap()
            .0;
        for (call, owner) in &requested {
            if call.name != "code_mode" {
                assert_eq!(
                    **owner,
                    OperationOwner::Tool {
                        call_id: parent.id.clone()
                    }
                );
            }
        }
        let results: Vec<_> = projection
            .records
            .iter()
            .filter_map(|record| match &record.entry {
                JournalEntry::ToolResultRecorded(result) => Some(&result.result),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 5);
        assert!(
            results
                .iter()
                .any(|result| result.output.contains(RAW_MARKER))
        );
        assert!(
            results
                .iter()
                .any(|result| !result.ok && result.call_id != parent.id)
        );
        let parent_result = results
            .iter()
            .find(|result| result.call_id == parent.id)
            .unwrap();
        assert_eq!(parent_result.ok, !script_error);
        assert!(parent_result.output.contains(SUMMARY));
        let requests: Vec<_> = projection
            .records
            .iter()
            .filter_map(|record| match &record.entry {
                JournalEntry::ModelRequestRecorded(model) => Some(&model.request),
                _ => None,
            })
            .collect();
        assert_eq!(requests.len(), 2);
        let next = serde_json::to_string(requests[1]).unwrap();
        assert!(next.contains(SUMMARY));
        assert!(
            !next.contains(RAW_MARKER),
            "nested data leaked into model request"
        );
        assert!(projection.unsettled_turns.is_empty());
        drop(runtime);
        // Replay must consume facts without reading files or executing JS again.
        for path in ["one.txt", "two.txt"] {
            std::fs::remove_file(root.path().join(path)).unwrap();
        }
        std::fs::remove_file(root.path().join("patched.txt")).unwrap();
        let replay = replay_workflow(
            &session,
            &config,
            &ModuleCatalog::from_config(&config).unwrap(),
            WorkflowReplayOptions::default(),
        )
        .await
        .unwrap();
        assert!(replay.comparison.matched, "{replay:#?}");
        assert!(replay.source_journal_unchanged);
        assert!(
            !root.path().join("patched.txt").exists(),
            "replay must not repeat the script's write"
        );
        assert_eq!(
            replay.recorded.status,
            if root_error {
                TurnSettlementStatus::Error
            } else {
                TurnSettlementStatus::Success
            }
        );
        assert_eq!(replay.recorded.status, replay.replay.status);
        config.tools.enabled.retain(|name| name != "code_mode");
        let disabled = registry(&config, root.path()).unwrap();
        assert!(disabled.tools.get("code_mode").is_none());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_cancel_and_timeout_settle_script_turns_and_allow_the_next_invocation() {
    for canceled in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let mut config = config("text('started'); while (true) {}", true);
        config.runtime.workflow_timeout_ms = if canceled { 10_000 } else { 1_000 };
        let runtime = Arc::new(
            AgentRuntime::builder(config, root.path().into())
                .with_config_path(Some(&root.path().join("config.json")))
                .build_async()
                .await
                .unwrap(),
        );
        let cancellation = CancellationToken::new();
        let running = tokio::spawn({
            let runtime = runtime.clone();
            let cancellation = cancellation.clone();
            async move {
                runtime
                    .run_with_cancellation("Run script".into(), cancellation)
                    .await
            }
        });
        if canceled {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if !runtime.session_dir().unwrap().is_dir() {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        continue;
                    }
                    let projection = SessionStore::open(runtime.session_dir().unwrap().to_owned())
                        .unwrap()
                        .load_projection()
                        .unwrap();
                    if projection.records.iter().any(|record| {
                        matches!(&record.entry,
                        JournalEntry::ToolCallRecorded(call) if call.call.name == "code_mode"
                            && matches!(call.phase, ToolCallRecordPhase::Resolved { .. }))
                    }) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            cancellation.cancel();
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(5), running)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        let session = runtime.session_dir().unwrap().to_owned();
        let result = runtime
            .execute_tool(
                ToolCall::new(
                    new_call_id(),
                    "code_mode",
                    json!({"code": "text('next script');"}),
                ),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(result.ok, "{result:?}");
        let projection = SessionStore::open(session)
            .unwrap()
            .load_projection()
            .unwrap();
        let settlement = projection
            .records
            .iter()
            .find_map(|record| match &record.entry {
                JournalEntry::TurnSettled(settled) => Some(settled.status),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            settlement,
            if canceled {
                TurnSettlementStatus::Canceled
            } else {
                TurnSettlementStatus::Timeout
            }
        );
        assert!(projection.unsettled_turns.is_empty());
        assert_eq!(projection.history, runtime.history().await.unwrap());
    }
}
