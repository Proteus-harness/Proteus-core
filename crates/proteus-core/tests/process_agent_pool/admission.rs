use super::*;
use async_trait::async_trait;
use proteus_contracts::{
    contracts::{CancellationToken, EventEmitter, EventSink},
    domain::EventEnvelope,
};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::sync::Notify;

struct BlockStarted {
    first: AtomicBool,
    entered: Notify,
}

#[async_trait]
impl EventSink for BlockStarted {
    async fn append(&self, event: EventEnvelope) -> anyhow::Result<()> {
        if matches!(event.event, Event::SubagentStarted { .. })
            && self.first.swap(false, Ordering::SeqCst)
        {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
        Ok(())
    }
}

#[tokio::test]
async fn already_cancelled_fresh_launch_never_leases_a_process() {
    let workspace = tempfile::tempdir().unwrap();
    // A lease would try to launch this nonexistent executable and return an
    // error, so Cancelled proves that admission stopped before process setup.
    let binary = workspace.path().join("must-not-be-launched");
    let runner = runner_from_json(json!({
        "binary":binary,"max_parallel":1,
        "roles":[{"name":"helper","description":"fixture","config":"unused.toml","max_processes":1}],
    }));
    let events = Arc::new(InMemoryEventStore::new());
    let ctx = test_runtime_context(events.clone());
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancelled_ctx = ctx.with_cancellation(cancellation);
    let request = AgentControlRequest::new(
        "helper",
        "must not run",
        AgentTask::new("parent", workspace.path().into()),
    );

    // Both branches are ready on every admission. Repetition exercises the
    // former randomized select without timing sleeps or a scheduler race.
    for _ in 0..64 {
        let result = runner
            .run(request.clone(), cancelled_ctx.clone())
            .await
            .expect("cancelled admission does not attempt process setup");
        assert_eq!(result.status, AgentLifecycleStatus::Cancelled);
        assert_eq!(result.metadata["resumable"], false);
    }
    let envelopes = events.envelopes().await;
    assert_eq!(
        envelopes
            .iter()
            .filter(|event| matches!(event.event, Event::SubagentStarted { .. }))
            .count(),
        64
    );
    assert_eq!(
        envelopes
            .iter()
            .filter(|event| matches!(
                &event.event,
                Event::SubagentFinished { status, iterations: 0, .. } if status == "cancelled"
            ))
            .count(),
        64
    );
}

#[tokio::test]
async fn already_cancelled_resume_preserves_retained_history() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let config = write_child_config(home.path());
    let runner = runner_from_json(json!({
        "binary":proteus_binary(),"max_parallel":1,"max_idle_processes":1,
        "roles":[{"name":"helper","description":"fixture","config":config,"timeout_ms":60000,"max_processes":1}],
    }));
    let ctx = test_runtime_context(Arc::new(InMemoryEventStore::new()));
    let task = AgentTask::new("parent", workspace.path().into());
    let first = runner
        .run(
            AgentControlRequest::new("helper", "first", task.clone()),
            ctx.clone(),
        )
        .await
        .expect("first turn");
    assert_eq!(first.metadata["resumable"], true);
    let child_thread_id = first.child_thread_id.unwrap();
    let resume = AgentControlRequest::new("helper", "resume", task)
        .with_metadata(json!({"task_id":child_thread_id.to_string()}));
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancelled_ctx = ctx.clone().with_cancellation(cancellation);

    for _ in 0..64 {
        let cancelled = runner
            .run(resume.clone(), cancelled_ctx.clone())
            .await
            .expect("cancel before leasing reserved peer");
        assert_eq!(cancelled.status, AgentLifecycleStatus::Cancelled);
        assert_eq!(cancelled.child_thread_id, Some(child_thread_id));
        assert_eq!(cancelled.metadata["resumable"], true);
    }
    let resumed = runner
        .run(resume, ctx)
        .await
        .expect("history still resumable");
    assert_eq!(resumed.status, AgentLifecycleStatus::Completed);
    assert_eq!(resumed.child_thread_id, Some(child_thread_id));
}

#[tokio::test]
async fn dropped_spawn_releases_pending_and_resume_reservations() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let config = write_child_config(home.path());
    let runner = runner_from_json(json!({
        "binary":proteus_binary(), "max_parallel":1,
        "roles":[{"name":"helper","description":"fixture","config":config,"timeout_ms":60000}],
    }));
    let ctx = test_runtime_context(Arc::new(InMemoryEventStore::new()));
    let task = AgentTask::new("parent", workspace.path().into());
    let request = AgentControlRequest::new("helper", "first", task.clone());
    let blocked = Arc::new(BlockStarted {
        first: AtomicBool::new(true),
        entered: Notify::new(),
    });
    let mut blocked_ctx = ctx.clone();
    blocked_ctx.events = Arc::new(EventEmitter::new(blocked.clone()));
    let child = {
        let runner = runner.clone();
        let request = request.clone();
        tokio::spawn(async move { runner.spawn(request, blocked_ctx).await })
    };
    blocked.entered.notified().await;
    child.abort();
    let _ = child.await;
    let first = runner
        .run(request, ctx.clone())
        .await
        .expect("slot released after dropped spawn");
    let task_id = first.child_thread_id.unwrap().to_string();
    let resume = AgentControlRequest::new("helper", "resume", task)
        .with_metadata(json!({"task_id":task_id}));
    let blocked = Arc::new(BlockStarted {
        first: AtomicBool::new(true),
        entered: Notify::new(),
    });
    let mut blocked_ctx = ctx.clone();
    blocked_ctx.events = Arc::new(EventEmitter::new(blocked.clone()));
    let child = {
        let runner = runner.clone();
        let resume = resume.clone();
        tokio::spawn(async move { runner.spawn(resume, blocked_ctx).await })
    };
    blocked.entered.notified().await;
    child.abort();
    let _ = child.await;
    let resumed = runner
        .run(resume, ctx)
        .await
        .expect("resume reservation restored");
    assert_eq!(resumed.status, AgentLifecycleStatus::Completed);
    assert_eq!(resumed.child_thread_id, first.child_thread_id);
}

#[cfg(unix)]
#[tokio::test]
async fn nonreading_peer_is_bounded_during_initial_write_and_releases_permit() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("peer.sh");
    std::fs::write(&binary, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    for canceled in [false, true] {
        let runner = runner_from_json(json!({
            "binary":binary,"max_parallel":1,"cancel_grace_ms":30,
            "roles":[{"name":"helper","description":"fixture","config":"unused.toml","timeout_ms": if canceled { 10000 } else { 60 },"max_processes":1}],
        }));
        let ctx = test_runtime_context(Arc::new(InMemoryEventStore::new()));
        let cancel = ctx.execution.scope.cancellation.clone();
        let request = AgentControlRequest::new(
            "helper",
            "x".repeat(1024 * 1024),
            AgentTask::new("parent", dir.path().into()),
        );
        let handle = runner.spawn(request.clone(), ctx.clone()).await.unwrap();
        if canceled {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel.cancel();
        }
        let result = tokio::time::timeout(Duration::from_secs(2), runner.wait(&handle))
            .await
            .expect("bounded stdin admission")
            .unwrap();
        assert_eq!(
            result.status,
            if canceled {
                AgentLifecycleStatus::Cancelled
            } else {
                AgentLifecycleStatus::TimedOut
            }
        );
        assert_eq!(result.metadata["resumable"], false);
        let next_ctx = test_runtime_context(Arc::new(InMemoryEventStore::new()));
        let next = runner
            .spawn(request, next_ctx)
            .await
            .expect("pending slot and permit released");
        runner.cancel(&next).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), runner.wait(&next))
            .await
            .unwrap()
            .unwrap();
    }
}
