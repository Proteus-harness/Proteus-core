//! Root aliasing must not flatten descendant identity, including during settle.
use super::*;
use proteus_contracts::{
    contracts::{
        ApprovalRequest, ApprovalResponse, ApprovalTransport, UserInputRequest, UserInputResponse,
        UserInputTransport,
    },
    domain::Event,
};
use std::sync::Mutex;

#[derive(Default)]
struct Capture {
    inputs: Mutex<Vec<UserInputRequest>>,
    approvals: Mutex<Vec<ApprovalRequest>>,
}
#[async_trait::async_trait]
impl UserInputTransport for Capture {
    fn can_request_user_input(&self) -> bool {
        true
    }
    async fn request_user_input(
        &self,
        request: UserInputRequest,
    ) -> anyhow::Result<UserInputResponse> {
        self.inputs.lock().unwrap().push(request);
        Ok(UserInputResponse::empty())
    }
}
#[async_trait::async_trait]
impl ApprovalTransport for Capture {
    fn can_request_approval(&self) -> bool {
        true
    }
    async fn request_approval(&self, request: ApprovalRequest) -> anyhow::Result<ApprovalResponse> {
        self.approvals.lock().unwrap().push(request);
        Ok(ApprovalResponse::approve())
    }
}
fn runner(root: &std::path::Path) -> Arc<dyn AgentControl> {
    use std::os::unix::fs::PermissionsExt;
    let binary = root.join("origin-peer.py");
    std::fs::write(&binary, include_str!("../fixtures/agent_origin_peer.py")).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    runner_from_json(json!({"binary":binary,"max_idle_processes":1,"roles":[{
        "name":"helper","config":"unused","description":"origin peer","max_processes":1,"timeout_ms":30000}]}))
}

#[tokio::test]
async fn descendant_origin_survives_resume_and_session_rotation() {
    let root = tempfile::tempdir().unwrap();
    let runner = runner(root.path());
    let events = Arc::new(InMemoryEventStore::new());
    let mut ctx = agent_context::test_runtime_context(events.clone());
    let capture = Arc::new(Capture::default());
    ctx.user_input = capture.clone();
    ctx.execution.approval = capture.clone();
    let mut previous: Option<(String, serde_json::Value)> = None;
    for generation in 0..3 {
        let mut request = AgentControlRequest::new(
            "helper",
            "normal",
            AgentTask::new("parent", root.path().into()),
        );
        if generation == 1 {
            request = request.with_metadata(json!({"task_id":previous.as_ref().unwrap().0}));
        }
        let result = runner.run(request, ctx.clone()).await.unwrap();
        let logical = result.child_thread_id.unwrap().to_string();
        let source: serde_json::Value = serde_json::from_str(&result.summary).unwrap();
        let envelopes = events.envelopes().await;
        for (name, expected_thread, expected_turn) in [
            ("root", logical.clone(), source["turn"].as_str().unwrap()),
            (
                "descendant",
                source["descendant"].as_str().unwrap().to_owned(),
                source["descendant_turn"].as_str().unwrap(),
            ),
        ] {
            let id = format!("{name}-{}", source["turn"].as_str().unwrap());
            let forwarded = envelopes
                .iter()
                .find(|e| matches!(&e.event,Event::ToolCallRequested{call} if call.id==id))
                .unwrap();
            assert_eq!(forwarded.thread_id.to_string(), expected_thread, "{name}");
            assert_eq!(
                forwarded.turn_id.unwrap().to_string(),
                expected_turn,
                "source turn must survive"
            );
            assert_eq!(
                forwarded.session_id, ctx.session_id,
                "aggregate belongs to receiving session"
            );
        }
        let origins: Vec<_> = capture
            .inputs
            .lock()
            .unwrap()
            .iter()
            .skip(generation * 4)
            .map(|r| r.origin.clone().unwrap())
            .chain(
                capture
                    .approvals
                    .lock()
                    .unwrap()
                    .iter()
                    .skip(generation * 4)
                    .map(|r| r.origin.clone().unwrap()),
            )
            .collect();
        assert_eq!(origins.len(), 8);
        for (index, origin) in origins.iter().enumerate() {
            if index % 4 == 2 {
                assert_eq!(
                    origin.execution_id.to_string(),
                    source["execution"].as_str().unwrap()
                );
                assert_eq!(origin.thread_id, None);
                assert_eq!(origin.turn_id, None);
                assert_eq!(origin.label.as_deref(), Some("standalone"));
                continue;
            }
            if index % 4 == 3 {
                assert_eq!(origin.execution_id, ctx.execution.scope.execution_id);
                assert_eq!(origin.thread_id.unwrap().to_string(), logical);
                assert_eq!(origin.turn_id, Some(ctx.turn_id));
                assert_eq!(origin.label.as_deref(), Some("helper"));
                continue;
            }
            let root = index % 4 == 0;
            assert_eq!(
                origin.thread_id.unwrap().to_string(),
                if root {
                    logical.clone()
                } else {
                    source["descendant"].as_str().unwrap().to_owned()
                }
            );
            assert_eq!(
                origin.turn_id.unwrap().to_string(),
                source[if root { "turn" } else { "descendant_turn" }]
                    .as_str()
                    .unwrap()
            );
            assert_eq!(
                origin.execution_id.to_string(),
                source[if root {
                    "execution"
                } else {
                    "descendant_execution"
                }]
                .as_str()
                .unwrap()
            );
            assert_eq!(
                origin.label.as_deref(),
                Some(if root { "helper" } else { "leaf" })
            );
        }
        if let Some((old_logical, old)) = &previous {
            assert_eq!(source["pid"], old["pid"], "reuse OS process");
            if generation == 1 {
                assert_eq!(&logical, old_logical);
                assert_eq!(source["root"], old["root"]);
                assert_eq!(source["session"], old["session"]);
            } else {
                assert_ne!(&logical, old_logical);
                assert_ne!(source["root"], old["root"]);
                assert_ne!(source["session"], old["session"]);
            }
            assert_ne!(source["turn"], old["turn"]);
        }
        previous = Some((logical, source));
    }
}

#[tokio::test]
async fn descendant_origin_survives_cancellation_settlement() {
    let root = tempfile::tempdir().unwrap();
    let runner = runner(root.path());
    let events = Arc::new(InMemoryEventStore::new());
    let ctx = agent_context::test_runtime_context(events.clone());
    let handle = runner
        .spawn(
            AgentControlRequest::new(
                "helper",
                "cancel",
                AgentTask::new("parent", root.path().into()),
            ),
            ctx,
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !root.path().join("ready").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    runner.cancel(&handle).await.unwrap();
    let result = runner.wait(&handle).await.unwrap();
    assert_eq!(result.status, AgentLifecycleStatus::Cancelled);
    let envelopes = events.envelopes().await;
    let source = envelopes
        .iter()
        .find_map(|e| match &e.event {
            Event::ToolCallRequested { call } => Some(&call.args),
            _ => None,
        })
        .unwrap();
    let settled = envelopes
        .iter()
        .find(|e| matches!(&e.event,Event::Error{message} if message=="descendant settlement"))
        .unwrap();
    assert_eq!(
        settled.thread_id.to_string(),
        source["descendant"].as_str().unwrap()
    );
    assert_eq!(
        settled.turn_id.unwrap().to_string(),
        source["descendant_turn"].as_str().unwrap()
    );
}
