use super::*;
use proteus_contracts::{domain::new_thread_id, model_standard::CanonicalMessage};
use proteus_core::core::HistoryMutationKind;
use serde_json::Value;

const KEPT_USER: &str = "KEPT USER PROMPT: preserve these exact instructions.";

#[derive(Clone, Copy)]
enum Compactor {
    PythonSuffix,
    Codex,
}

impl Compactor {
    fn id(self) -> &'static str {
        match self {
            Self::PythonSuffix => "python_suffix",
            Self::Codex => "codex",
        }
    }
}

fn text(messages: &[CanonicalMessage]) -> String {
    messages
        .iter()
        .map(CanonicalMessage::display_text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn captures(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn state_files(directory: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn composition_config(kind: Compactor, capture: &Path, state: &Path) -> AppConfig {
    let worker = PathBuf::from(
        std::env::var_os("PROTEUS_TEST_DCP_MODULE").expect("scripts/test.py prepares DCP"),
    );
    let mut config: AppConfig = serde_json::from_value(json!({
        "active_provider": "dcp-probe",
        "providers": {"dcp-probe": {"provider":"dcp-probe", "model":"probe", "stream":true}},
        "modules": {"workflow":"coding.codex_loop", "context":"codex_context", "policy":"allow_all", "compactor":kind.id(), "hooks":["hook.dcp"]},
        "tools": {"enabled":["compress","dcp"]},
        "components": {
            "reference-agent": {"command":test_model::reference_module(), "exports": {
                "workflow":{"coding.codex_loop":{}}, "context":{"codex_context":{}}, "policy":{"allow_all":{}}, "compactor":{"codex":{}}
            }},
            "dcp": {"command":"node", "args":[worker], "exports":{"hook":{"hook.dcp":{}},"tool":{"dcp.tools":{}}}},
            "probe": {"command":"python3", "args":["-B",workspace_file("crates/proteus-core/tests/fixtures/dcp_compaction_model.py")], "exports":{"model":{"dcp-probe":{}}}}
        },
        "module_config": {
            "context":{"codex_context":{"providers":[]}},
            "hook":{"hook.dcp":{"state_dir":state}}, "tool":{"dcp.tools":{"state_dir":state}},
            "model":{"dcp-probe":{
                "descriptor":ProcessModelDescriptor{adapter_id:"dcp-probe".into(),capabilities:ModelCapabilities::basic_text_and_tools().with_streaming(true),hosted_tools:vec![]},
                "capture":capture
            }},
            "compactor":{"codex":{"trigger_tokens":2000},"python_suffix":{"trigger_messages":7,"retain_user_turns":1}}
        },
        "runtime":{"model_timeout_ms":5000,"workflow_timeout_ms":15000}
    })).unwrap();
    if matches!(kind, Compactor::PythonSuffix) {
        config.components.insert("suffix".into(), serde_json::from_value(json!({
            "command":"python3", "args":["-B",workspace_file("examples/modules/compactor-process/compact.py")], "exports":{"compactor":{"python_suffix":{}}}
        })).unwrap());
    }
    config
}

async fn check_composition(kind: Compactor) {
    let workspace = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let capture = workspace.path().join("requests.jsonl");
    let config = composition_config(kind, &capture, state.path());
    let config_path = workspace.path().join("config.json");
    std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    let thread_id = new_thread_id();
    let runtime = AgentRuntime::builder(config.clone(), workspace.path().into())
        .with_config_path(Some(&config_path))
        .with_session_ids(proteus_contracts::domain::new_session_id(), thread_id)
        .build_async()
        .await
        .unwrap();
    runtime
        .run("OLD USER PROMPT: research now obsolete.".into())
        .await
        .unwrap();
    runtime.run(KEPT_USER.into()).await.unwrap();
    let session_dir = runtime.session_dir().unwrap().to_path_buf();
    let source = SessionStore::open(session_dir.clone())
        .unwrap()
        .load_messages()
        .unwrap();
    let kept_id = source
        .iter()
        .find(|message| message.display_text() == KEPT_USER)
        .unwrap()
        .id;
    let anchor_id = source.first().unwrap().id;
    let recorded = captures(&capture);
    assert_eq!(
        recorded.len(),
        5,
        "compaction must wait until DCP has compressed the range"
    );
    let compressed: Vec<CanonicalMessage> =
        serde_json::from_value(recorded[4]["request"]["messages"].clone()).unwrap();
    assert!(text(&compressed).contains("DCP RANGE SUMMARY."));
    assert!(!text(&compressed).contains(KEPT_USER));
    assert!(
        text(&source).contains(KEPT_USER),
        "DCP must keep canonical user source"
    );
    assert_replay(&runtime, &config).await;

    runtime
        .run("Continue after actual compaction.".into())
        .await
        .unwrap();
    let journal = records(&runtime);
    let checkpoint = journal
        .iter()
        .find_map(|record| match &record.entry {
            JournalEntry::HistoryMutated(mutation)
                if mutation
                    .compaction
                    .as_ref()
                    .is_some_and(|report| report.changed) =>
            {
                Some(mutation)
            }
            _ => None,
        })
        .expect("real compactor replacement was checkpointed");
    assert_eq!(checkpoint.mutation, HistoryMutationKind::Checkpoint);
    assert!(
        checkpoint
            .messages
            .iter()
            .any(|message| message.id == kept_id && message.display_text() == KEPT_USER)
    );
    if matches!(kind, Compactor::PythonSuffix) {
        assert!(
            !checkpoint
                .messages
                .iter()
                .any(|message| message.id == anchor_id),
            "suffix compaction must cut the DCP start anchor"
        );
    } else {
        assert!(text(&checkpoint.messages).contains("COMPACTOR HANDOFF"));
        assert!(!text(&checkpoint.messages).contains("Finished obsolete research"));
    }
    let outgoing = journal
        .iter()
        .rev()
        .find_map(|record| match &record.entry {
            JournalEntry::ModelRequestRecorded(model) => Some(&model.request.messages),
            _ => None,
        })
        .unwrap();
    assert!(
        text(outgoing).contains(KEPT_USER),
        "stale DCP range must not hide the compactor's retained user"
    );
    assert!(!text(outgoing).contains("DCP RANGE SUMMARY."));
    assert_replay(&runtime, &config).await;
    let before_restart = SessionStore::open(session_dir.clone())
        .unwrap()
        .load_messages()
        .unwrap();
    let old_pid = captures(&capture).last().unwrap()["pid"].clone();
    drop(runtime);

    let cold = AgentRuntime::builder(config.clone(), workspace.path().into())
        .with_config_path(Some(&config_path))
        .resume_from_session_dir(session_dir.clone(), thread_id)
        .unwrap()
        .build_async()
        .await
        .unwrap();
    assert_eq!(
        SessionStore::open(session_dir.clone())
            .unwrap()
            .load_messages()
            .unwrap(),
        before_restart
    );
    cold.run("Continue after cold runtime and component restart.".into())
        .await
        .unwrap();
    let recorded = captures(&capture);
    assert_ne!(
        recorded.last().unwrap()["pid"],
        old_pid,
        "the provider process must be freshly launched"
    );
    let outgoing: Vec<CanonicalMessage> =
        serde_json::from_value(recorded.last().unwrap()["request"]["messages"].clone()).unwrap();
    assert!(text(&outgoing).contains(KEPT_USER));
    assert!(!text(&outgoing).contains("DCP RANGE SUMMARY."));
    let requests_before_replay = std::fs::read(&capture).unwrap();
    let state_before_replay = state_files(state.path());
    assert_eq!(
        state_before_replay.len(),
        1,
        "restart keeps the same session-owned DCP state"
    );
    assert_replay(&cold, &config).await;
    assert_eq!(
        std::fs::read(&capture).unwrap(),
        requests_before_replay,
        "replay cannot repeat model completions"
    );
    assert_eq!(
        state_files(state.path()),
        state_before_replay,
        "replay cannot repeat DCP storage effects"
    );
    assert_eq!(records(&cold).iter().filter(|record| matches!(&record.entry, JournalEntry::ToolResultRecorded(result) if result.result.ok && result.result.output.contains("Compressed"))).count(), 1, "compression executes exactly once across restart and replay");
    drop(cold);
    let server = proteus_core::app_server::AgentAppServer::launch_resumed(
        config,
        workspace.path().into(),
        Some(&config_path),
        session_dir,
    )
    .await
    .unwrap();
    assert!(
        server
            .transcript()
            .await
            .unwrap()
            .iter()
            .any(|item| item.text == KEPT_USER),
        "cold history retains the exact original user source"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn dcp_python_suffix_compaction_preserves_retained_user_after_checkpoint_and_cold_restart() {
    check_composition(Compactor::PythonSuffix).await;
}

#[tokio::test]
async fn dcp_codex_compaction_preserves_retained_user_after_checkpoint_and_cold_restart() {
    check_composition(Compactor::Codex).await;
}
