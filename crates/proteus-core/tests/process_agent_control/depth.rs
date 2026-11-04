//! Real self-cycle and A -> B -> A peers cannot reset an inherited budget.
use super::*;

fn child_config(path: &std::path::Path, next: &std::path::Path) {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recursive_agent.py");
    let policy = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/process_allow_all_policy.py");
    let contents = format!(
        r#"active_provider = "fake"
[providers.fake]
provider = "fake"
model = "fake-tool-model"
[modules]
workflow = "recursive_probe"
policy = "fixture_allow_all"
[components.recursive]
command = "python3"
args = [{}]
[components.recursive.exports.workflow.recursive_probe]
[components.policy]
command = "python3"
args = [{}]
[components.policy.exports.policy.fixture_allow_all]
[agent_control]
max_depth = 1000
max_idle_processes = 0
[[agent_control.roles]]
name = "next"
description = "recursive test peer"
config = {}
timeout_ms = 60000
"#,
        serde_json::to_string(&fixture).unwrap(),
        serde_json::to_string(&policy).unwrap(),
        serde_json::to_string(next).unwrap()
    );
    std::fs::write(path, contents + &test_model::toml_component()).unwrap();
}

#[tokio::test]
async fn process_peers_inherit_depth_budget_across_self_and_two_profile_cycles() {
    for (cycle, budget) in [(false, 1), (true, 2)] {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let first = home.path().join("a.toml");
        let second = home.path().join("b.toml");
        child_config(&first, if cycle { &second } else { &first });
        if cycle {
            child_config(&second, &first);
        }
        let runner = runner_from_json(json!({
            "binary": proteus_binary(), "max_depth": budget, "max_idle_processes": 0,
            "roles": [{"name":"next", "description":"first peer", "config":first, "timeout_ms":60000}]
        }));
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            runner.run(
                AgentControlRequest::new(
                    "next",
                    "3",
                    AgentTask::new("bounded recursion", workspace.path().into()),
                )
                .with_metadata(json!({"agent_control_depth":0})),
                test_runtime_context(),
            ),
        )
        .await
        .expect("bounded child recursion")
        .unwrap();
        assert!(
            result.summary.contains("depth limit reached"),
            "{}",
            result.summary
        );
        let records =
            std::fs::read_to_string(workspace.path().join("recursive-peers.jsonl")).unwrap();
        let peers: Vec<serde_json::Value> = records
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            peers.len(),
            budget as usize,
            "forbidden descendants must not reach their workflow"
        );
        let pids: std::collections::HashSet<_> = peers
            .iter()
            .map(|peer| peer["pid"].as_u64().unwrap())
            .collect();
        assert_eq!(
            pids.len(),
            budget as usize,
            "each peer must be a separate actual Proteus process"
        );
    }
}
