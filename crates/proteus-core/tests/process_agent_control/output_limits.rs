use std::{path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use proteus_contracts::contracts::{
    ApprovalRequest, ApprovalResponse, ApprovalTransport, UserInputRequest, UserInputResponse,
    UserInputTransport,
};
use tokio::sync::Notify;

use super::*;

struct DelayedInteraction {
    waiting: PathBuf,
    release: Notify,
}

impl DelayedInteraction {
    async fn wait(&self) {
        std::fs::write(&self.waiting, "waiting").expect("mark pending interaction");
        self.release.notified().await;
    }
}

#[async_trait]
impl ApprovalTransport for DelayedInteraction {
    fn can_request_approval(&self) -> bool {
        true
    }

    async fn request_approval(
        &self,
        _request: ApprovalRequest,
    ) -> anyhow::Result<ApprovalResponse> {
        self.wait().await;
        Ok(ApprovalResponse::approve())
    }
}

#[async_trait]
impl UserInputTransport for DelayedInteraction {
    fn can_request_user_input(&self) -> bool {
        true
    }

    async fn request_user_input(
        &self,
        _request: UserInputRequest,
    ) -> anyhow::Result<UserInputResponse> {
        self.wait().await;
        Ok(UserInputResponse::empty())
    }
}

fn runner(root: &std::path::Path) -> Arc<dyn AgentControl> {
    runner_with_timeout(root, None)
}

fn runner_with_timeout(root: &std::path::Path, timeout_ms: Option<u64>) -> Arc<dyn AgentControl> {
    use std::os::unix::fs::PermissionsExt;
    let binary = root.join("peer.py");
    std::fs::write(&binary, include_str!("../fixtures/agent_output_peer.py")).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    runner_from_json(json!({
        "binary": binary,
        "max_parallel": 2,
        "cancel_grace_ms": 100,
        "roles": [{
            "name": "helper", "description": "Output limit fixture",
            "config": "unused", "parallel_safe": true, "max_processes": 2,
            "timeout_ms": timeout_ms
        }]
    }))
}

async fn wait_for(path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("peer reached barrier");
}

#[tokio::test]
async fn output_failure_interrupts_pending_interaction_and_preserves_sibling() {
    for (mode, expected) in [
        ("approval-overflow", "256 buffered outputs"),
        ("input-overflow", "256 buffered outputs"),
        ("bytes-overflow", "33554432 buffered bytes"),
        ("frame-overflow", "frame exceeded 8388608 bytes"),
        ("malformed-output", "invalid subagent stdout JSONL output"),
        ("stdout-closed", "subagent child stdout closed"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let runner = runner(root.path());
        let mut ctx = test_runtime_context();
        let interaction = Arc::new(DelayedInteraction {
            waiting: root.path().join("waiting"),
            release: Notify::new(),
        });
        ctx.execution.approval = interaction.clone();
        ctx.user_input = interaction;
        let task = AgentTask::new("output regression", root.path().to_path_buf());
        let flooded = runner
            .spawn(
                AgentControlRequest::new("helper", mode, task.clone()),
                ctx.clone(),
            )
            .await
            .unwrap();
        wait_for(&root.path().join("waiting")).await;
        wait_for(&root.path().join("flood-started")).await;
        let sibling = runner
            .spawn(AgentControlRequest::new("helper", "healthy", task), ctx)
            .await
            .unwrap();
        let error = tokio::time::timeout(Duration::from_secs(5), runner.wait(&flooded))
            .await
            .expect("overflow must interrupt the pending interaction")
            .expect_err("overflow must fail the peer");
        assert!(format!("{error:#}").contains(expected), "{mode}: {error:#}");
        assert!(!root.path().join("answered").exists());
        let result = tokio::time::timeout(Duration::from_secs(5), runner.wait(&sibling))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.status, AgentLifecycleStatus::Completed);
        assert_eq!(result.summary, "healthy peer");
    }
}

#[tokio::test]
async fn queued_output_preserves_approval_result_and_targeted_cancel() {
    for cancel in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let runner = runner(root.path());
        let mut ctx = test_runtime_context();
        let interaction = Arc::new(DelayedInteraction {
            waiting: root.path().join("waiting"),
            release: Notify::new(),
        });
        ctx.execution.approval = interaction.clone();
        let handle = runner
            .spawn(
                AgentControlRequest::new(
                    "helper",
                    "normal",
                    AgentTask::new("normal", root.path().to_path_buf()),
                ),
                ctx,
            )
            .await
            .unwrap();
        wait_for(&root.path().join("produced")).await;
        if cancel {
            runner.cancel(&handle).await.unwrap();
        } else {
            interaction.release.notify_one();
        }
        let result = tokio::time::timeout(Duration::from_secs(5), runner.wait(&handle))
            .await
            .expect("control response must not be blocked by queued output")
            .unwrap();
        if cancel {
            assert_eq!(result.status, AgentLifecycleStatus::Cancelled);
            assert_eq!(
                result.summary,
                (0..32).map(|i| format!("{i:02};")).collect::<String>()
            );
            assert!(root.path().join("cancelled").exists());
            assert!(!root.path().join("answered").exists());
        } else {
            assert_eq!(result.status, AgentLifecycleStatus::Completed);
            assert_eq!(result.summary, "approved result");
        }
    }
}

#[tokio::test]
async fn role_deadline_cancels_pending_interaction_and_reuses_settled_peer() {
    for mode in ["normal", "input-timeout"] {
        let root = tempfile::tempdir().unwrap();
        let runner = runner_with_timeout(root.path(), Some(600));
        let mut ctx = test_runtime_context();
        let interaction = Arc::new(DelayedInteraction {
            waiting: root.path().join("waiting"),
            release: Notify::new(),
        });
        ctx.execution.approval = interaction.clone();
        ctx.user_input = interaction;
        let task = AgentTask::new("deadline", root.path().to_path_buf());
        let handle = runner
            .spawn(
                AgentControlRequest::new("helper", mode, task.clone()),
                ctx.clone(),
            )
            .await
            .unwrap();
        wait_for(&root.path().join("produced")).await;
        let result = tokio::time::timeout(Duration::from_secs(3), runner.wait(&handle))
            .await
            .expect("bounded role timeout and settlement")
            .unwrap();
        assert_eq!(result.status, AgentLifecycleStatus::TimedOut);
        assert_eq!(result.metadata["resumable"], true);
        assert!(root.path().join("cancelled").exists());
        assert!(!root.path().join("answered").exists());
        if mode == "normal" {
            assert_eq!(
                result.summary,
                (0..32).map(|i| format!("{i:02};")).collect::<String>()
            );
        }
        let resumed = runner
            .run(
                AgentControlRequest::new("helper", "healthy", task)
                    .with_metadata(json!({"task_id": handle.child_thread_id.to_string()})),
                ctx,
            )
            .await
            .expect("settled peer is resumable");
        assert_eq!(resumed.status, AgentLifecycleStatus::Completed);
        assert_eq!(resumed.summary, "healthy peer");
        let requests = requests(root.path());
        assert_eq!(requests.len(), 3, "Send, exact Cancel, resumed Send");
        assert_eq!(
            requests[1]["request"]["target_id"],
            requests[0]["request"]["id"]
        );
        assert_eq!(requests[0]["pid"], requests[2]["pid"]);
    }
}

fn requests(root: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(root.join("requests.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

struct BlockSettlementEvents(PathBuf);

#[async_trait]
impl proteus_contracts::contracts::EventSink for BlockSettlementEvents {
    async fn append(
        &self,
        envelope: proteus_contracts::domain::EventEnvelope,
    ) -> anyhow::Result<()> {
        if matches!(
            envelope.event,
            proteus_contracts::domain::Event::Error { .. }
        ) {
            std::fs::write(&self.0, "entered").unwrap();
            std::future::pending::<()>().await;
        }
        Ok(())
    }
}

#[tokio::test]
async fn role_deadline_does_not_pool_unsettled_or_partially_written_peer() {
    for mode in ["ignore-cancel", "blocked-settlement-event", "stop-reading"] {
        let root = tempfile::tempdir().unwrap();
        let runner = runner_with_timeout(root.path(), Some(600));
        let mut ctx = test_runtime_context();
        ctx.execution.approval = Arc::new(DelayedInteraction {
            waiting: root.path().join("waiting"),
            release: Notify::new(),
        });
        if mode == "blocked-settlement-event" {
            ctx.events = Arc::new(proteus_contracts::contracts::EventEmitter::new(Arc::new(
                BlockSettlementEvents(root.path().join("event-blocked")),
            )));
        }
        let handle = runner
            .spawn(
                collaboration_request(
                    "helper",
                    "alpha",
                    mode,
                    AgentTask::new("deadline", root.path().to_path_buf()),
                ),
                ctx,
            )
            .await
            .unwrap();
        wait_for(&root.path().join("produced")).await;
        if mode == "stop-reading" {
            // Four valid 16 KB envelopes encode above normal pipe capacity;
            // the peer has stopped reading, so delivery cannot finish.
            for _ in 0..4 {
                runner
                    .send(&handle, agent_message("alpha", &"\nx".repeat(8_000)))
                    .await
                    .unwrap();
            }
        }
        let result = tokio::time::timeout(Duration::from_secs(3), runner.wait(&handle))
            .await
            .expect("the entire grace including forwarding is bounded")
            .unwrap();
        assert_eq!(result.status, AgentLifecycleStatus::TimedOut);
        assert_eq!(result.metadata["resumable"], false, "{mode}");
        if mode == "blocked-settlement-event" {
            assert!(root.path().join("event-blocked").exists());
        }
        if mode != "stop-reading" {
            assert!(root.path().join("cancelled").exists(), "{mode}");
            let requests = requests(root.path());
            assert_eq!(
                requests[1]["request"]["target_id"],
                requests[0]["request"]["id"]
            );
        }
    }
}
