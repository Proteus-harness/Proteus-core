use super::super::{TASK_TOOL, TaskTool};
use super::*;
use crate::{
    contracts::{AgentLifecycleStatus, ExecutionAttribution, Tool, ToolContext},
    domain::{AgentTask, ToolCall, new_execution_id, new_session_id, new_thread_id},
};
use async_trait::async_trait;
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::sync::Notify;

struct GatedHost {
    session: SessionId,
    child_thread: crate::domain::ThreadId,
    cancellation: CancellationToken,
    release: CancellationToken,
    entered: Notify,
    calls: AtomicUsize,
    write_before_terminal: bool,
    cancel_on_session_lookup: bool,
    terminal_status: AgentLifecycleStatus,
    fail_terminal: bool,
}

#[async_trait]
impl AgentControlToolHost for GatedHost {
    fn session_id(&self) -> Option<SessionId> {
        if self.cancel_on_session_lookup {
            self.cancellation.cancel();
        }
        Some(self.session)
    }

    async fn run_agent(&self, request: AgentControlRequest) -> Result<AgentControlResult> {
        if self.calls.fetch_add(1, Ordering::SeqCst) != 0 {
            anyhow::bail!("task_id belongs to a subagent that is still running");
        }
        self.entered.notify_one();
        self.release.cancelled().await;
        if self.fail_terminal {
            anyhow::bail!("child failed without a settlement proof");
        }
        if self.write_before_terminal {
            fs::write(request.task.cwd.join("child.txt"), "retained child work\n")?;
        }
        Ok(AgentControlResult::new("settled", self.terminal_status)
            .with_child_thread_id(self.child_thread)
            .with_metadata(json!({"resumable":true})))
    }
}

fn init_repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Test"],
    ] {
        git(repo.path(), &args);
    }
    fs::write(repo.path().join("base.txt"), "base\n").unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-q", "-m", "base"]);
    repo
}

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn context(cwd: PathBuf, host: Arc<GatedHost>) -> ToolContext {
    let mut ctx = ToolContext::new(
        cwd.clone(),
        ExecutionAttribution::detached(new_execution_id()),
    );
    ctx.task = Some(AgentTask::new("parent", cwd));
    ctx.cancellation = host.cancellation.clone();
    ctx.agent_control = Some(host);
    ctx
}

fn role() -> AgentProfile {
    AgentProfile::new("coder", "worktree owner").with_isolation(AgentIsolation::Worktree)
}

fn host(cancellation: CancellationToken, write_before_terminal: bool) -> Arc<GatedHost> {
    Arc::new(GatedHost {
        session: new_session_id(),
        child_thread: new_thread_id(),
        cancellation,
        release: CancellationToken::new(),
        entered: Notify::new(),
        calls: AtomicUsize::new(0),
        write_before_terminal,
        cancel_on_session_lookup: false,
        terminal_status: AgentLifecycleStatus::Completed,
        fail_terminal: false,
    })
}

#[tokio::test]
async fn concurrent_resume_cannot_clean_the_active_owners_worktree() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let repo = init_repo();
        let host = host(CancellationToken::new(), false);
        let info = git_workspace::create_worktree(repo.path(), "existing-owner").unwrap();
        let task_id = host.child_thread.to_string();
        // A retained checkout can be clean after the user reverts earlier work.
        workspaces().lock().unwrap().insert(
            task_id.clone(),
            WorkspaceRecord {
                info: Some(info.clone()),
                session_id: Some(host.session),
            },
        );
        let tool = TaskTool::new(vec![role()], 30_000);
        let call = ToolCall::new(
            "resume-owner",
            TASK_TOOL,
            json!({
                "agent_type":"coder", "prompt":"continue", "task_id":task_id,
            }),
        );
        let ctx = context(repo.path().into(), host.clone());
        let first = tool.invoke(&call, ctx.clone());
        tokio::pin!(first);
        tokio::select! {
            _ = host.entered.notified() => {}
            result = &mut first => panic!("child must remain active: {result:?}"),
        }
        let rejected = tool.invoke(&call, ctx).await.unwrap();
        assert!(!rejected.ok);
        assert!(rejected.error.unwrap().contains("active task"));
        assert_eq!(host.calls.load(Ordering::SeqCst), 1);
        assert!(
            info.path.is_dir(),
            "rejected resume must not clean active checkout"
        );
        host.release.cancel();
        let completed = first.await.unwrap();
        assert!(completed.ok);
        assert!(
            !completed.output.contains("[task_id:"),
            "backend-retained history cannot resume a removed checkout"
        );
        assert!(
            !info.path.exists(),
            "unchanged checkout is cleaned only after terminal"
        );
        assert!(!workspaces().lock().unwrap().contains_key(&task_id));
    })
    .await
    .expect("resume ownership scenario must settle");
}

#[tokio::test]
async fn dropped_waiter_retains_checkout_after_child_terminal() {
    tokio::time::timeout(Duration::from_secs(10), async {
        for dirty in [false, true] {
            let repo = init_repo();
            let parent = CancellationToken::new();
            let host = host(parent.child_token(), dirty);
            let tool = TaskTool::new(vec![role()], 30_000);
            let call = ToolCall::new(
                "dropped-owner",
                TASK_TOOL,
                json!({
                    "agent_type":"coder", "prompt":"do work",
                }),
            );
            let path = repo.path().join(".proteus/worktrees/coder-dropped-owner");
            {
                let pending = tool.invoke(&call, context(repo.path().into(), host.clone()));
                tokio::pin!(pending);
                tokio::select! {
                    _ = host.entered.notified() => {}
                    result = &mut pending => panic!("child must remain active: {result:?}"),
                }
            }
            assert!(host.cancellation.is_cancelled());
            assert!(
                !parent.is_cancelled(),
                "only the per-tool token is canceled"
            );
            assert!(path.is_dir(), "waiter drop is not child settlement");
            host.release.cancel();
            let task_id = host.child_thread.to_string();
            loop {
                let settled = workspaces()
                    .lock()
                    .unwrap()
                    .get(&task_id)
                    .is_some_and(|record| record.info.is_some());
                // Wait for the owner, not merely a published registry entry.
                if settled && Arc::strong_count(&host) == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(
                path.is_dir(),
                "terminal child receipt is not writer quiescence"
            );
            if dirty {
                assert_eq!(
                    fs::read_to_string(path.join("child.txt")).unwrap(),
                    "retained child work\n"
                );
            }
            remove_workspace(&task_id, Some(host.session));
        }
    })
    .await
    .expect("dropped waiter owner must eventually settle");
}

#[tokio::test]
async fn unsuccessful_dispatched_work_preserves_clean_checkout_without_guessing_resume() {
    tokio::time::timeout(Duration::from_secs(10), async {
        for fail_terminal in [false, true] {
            let repo = init_repo();
            let mut host = host(CancellationToken::new(), false);
            let settings = Arc::get_mut(&mut host).unwrap();
            settings.terminal_status = AgentLifecycleStatus::Cancelled;
            settings.fail_terminal = fail_terminal;
            host.release.cancel();
            let tool = TaskTool::new(vec![role()], 30_000);
            let call = ToolCall::new(
                "unsuccessful-owner",
                TASK_TOOL,
                json!({
                    "agent_type":"coder", "prompt":"do work",
                }),
            );
            let result = tool
                .invoke(&call, context(repo.path().into(), host.clone()))
                .await
                .unwrap();
            assert_eq!(result.ok, !fail_terminal);
            assert!(
                !host.cancellation.is_cancelled(),
                "normal completion disarms the waiter guard"
            );
            assert!(
                repo.path()
                    .join(".proteus/worktrees/coder-unsuccessful-owner")
                    .is_dir()
            );
            let task_id = host.child_thread.to_string();
            assert_eq!(
                workspaces().lock().unwrap().contains_key(&task_id),
                !fail_terminal,
                "only an explicit resumable result republishes the mapping"
            );
            remove_workspace(&task_id, Some(host.session));
        }
    })
    .await
    .expect("unsuccessful owner must retain its checkout");
}

#[tokio::test]
async fn canceled_owner_does_not_start_worktree_preparation_or_child_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let host = host(cancellation, false);
    let tool = TaskTool::new(vec![role()], 30_000);
    let call = ToolCall::new(
        "not-admitted",
        TASK_TOOL,
        json!({
            "agent_type":"coder", "prompt":"must not run",
        }),
    );
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        tool.invoke(&call, context(root.path().into(), host.clone())),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!result.ok);
    assert!(
        result
            .error
            .unwrap()
            .contains("before worktree preparation")
    );
    assert_eq!(host.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn cancellation_before_resume_dispatch_restores_idle_workspace() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let repo = init_repo();
        let mut host = host(CancellationToken::new(), false);
        // Deliver cancellation after run_owned's initial check. The resume
        // claim is synchronous; its next check must roll that claim back.
        Arc::get_mut(&mut host).unwrap().cancel_on_session_lookup = true;
        let info = git_workspace::create_worktree(repo.path(), "unstarted-resume").unwrap();
        let task_id = host.child_thread.to_string();
        workspaces().lock().unwrap().insert(
            task_id.clone(),
            WorkspaceRecord {
                info: Some(info.clone()),
                session_id: Some(host.session),
            },
        );
        let tool = TaskTool::new(vec![role()], 30_000);
        let call = ToolCall::new(
            "cancel-resume",
            TASK_TOOL,
            json!({
                "agent_type":"coder", "prompt":"continue", "task_id":task_id,
            }),
        );
        let result = tool
            .invoke(&call, context(repo.path().into(), host.clone()))
            .await
            .unwrap();
        assert!(!result.ok);
        assert!(result.error.unwrap().contains("before child dispatch"));
        assert_eq!(host.calls.load(Ordering::SeqCst), 0);
        assert!(
            info.path.is_dir(),
            "an unstarted resume must not remove its checkout"
        );
        assert!(
            workspaces()
                .lock()
                .unwrap()
                .get(&task_id)
                .is_some_and(|record| record.info.is_some())
        );
        remove_workspace(&task_id, Some(host.session));
    })
    .await
    .expect("unstarted resume must roll back");
}

#[tokio::test]
async fn late_effect_can_write_after_a_dropped_waiters_child_terminal() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let repo = init_repo();
        let host = host(CancellationToken::new(), false);
        let tool = TaskTool::new(vec![role()], 30_000);
        let call = ToolCall::new(
            "late-effect",
            TASK_TOOL,
            json!({
                "agent_type":"coder", "prompt":"do work",
            }),
        );
        let path = repo.path().join(".proteus/worktrees/coder-late-effect");
        let effect_entered = Notify::new();
        let effect_release = CancellationToken::new();
        // Model a separately owned tool effect: the child turn can report
        // terminal while this future still has a filesystem write pending.
        // Both fixture futures stay owned by join, including on timeout.
        let effect = async {
            effect_entered.notify_one();
            effect_release.cancelled().await;
            fs::write(path.join("late.txt"), "effect settled after child\n").unwrap();
        };
        let canceled_facade = async {
            {
                let pending = tool.invoke(&call, context(repo.path().into(), host.clone()));
                tokio::pin!(pending);
                tokio::select! {
                    _ = host.entered.notified() => {}
                    result = &mut pending => panic!("child must remain active: {result:?}"),
                }
                effect_entered.notified().await;
            }
            assert!(host.cancellation.is_cancelled());
            host.release.cancel();
            while Arc::strong_count(&host) != 1 {
                tokio::task::yield_now().await;
            }
            assert!(
                path.is_dir(),
                "child terminal must not delete a late effect's checkout"
            );
            effect_release.cancel();
        };
        tokio::join!(canceled_facade, effect);
        assert_eq!(
            fs::read_to_string(path.join("late.txt")).unwrap(),
            "effect settled after child\n"
        );
        remove_workspace(&host.child_thread.to_string(), Some(host.session));
    })
    .await
    .expect("late effect scenario must settle without checkout deletion");
}

#[tokio::test]
async fn canceled_fresh_preparation_cleans_checkout_without_dispatch() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let repo = init_repo();
        let mut host = host(CancellationToken::new(), false);
        Arc::get_mut(&mut host).unwrap().cancel_on_session_lookup = true;
        let tool = TaskTool::new(vec![role()], 30_000);
        let call = ToolCall::new(
            "unstarted-fresh",
            TASK_TOOL,
            json!({
                "agent_type":"coder", "prompt":"must not dispatch",
            }),
        );
        let result = tool
            .invoke(&call, context(repo.path().into(), host.clone()))
            .await
            .unwrap();
        assert!(!result.ok);
        assert!(result.error.unwrap().contains("before child dispatch"));
        assert_eq!(host.calls.load(Ordering::SeqCst), 0);
        assert!(host.cancellation.is_cancelled());
        assert!(
            !repo
                .path()
                .join(".proteus/worktrees/coder-unstarted-fresh")
                .exists()
        );
    })
    .await
    .expect("fresh preparation without dispatch can be cleaned");
}

#[tokio::test]
async fn missing_checkout_keeps_branch_without_advertising_a_worktree_resume() {
    tokio::time::timeout(Duration::from_secs(10), async {
        for allow_cleanup in [false, true] {
            let repo = init_repo();
            let host = host(CancellationToken::new(), false);
            let info = git_workspace::create_worktree(repo.path(), "branch-only").unwrap();
            fs::write(info.path.join("result.txt"), "retained branch\n").unwrap();
            git(&info.path, &["add", "."]);
            git(&info.path, &["commit", "-q", "-m", "child result"]);
            git(
                repo.path(),
                &["worktree", "remove", info.path.to_str().unwrap()],
            );
            let task_id = host.child_thread.to_string();
            workspaces().lock().unwrap().insert(
                task_id.clone(),
                WorkspaceRecord {
                    info: None,
                    session_id: Some(host.session),
                },
            );
            let prepared = PreparedWorkspace {
                info: info.clone(),
                resume_task_id: Some(task_id.clone()),
                session_id: Some(host.session),
            };
            let result = AgentControlResult::new("done", AgentLifecycleStatus::Completed)
                .with_child_thread_id(host.child_thread)
                .with_metadata(json!({"resumable": true}));
            let policy =
                allow_cleanup.then(|| CleanupPolicy::CompletedTurn(host.cancellation.clone()));
            let finalized = finalize_workspace(Some(prepared), &result, policy).await;
            assert!(finalized.task_id.is_none());
            assert_eq!(
                result.metadata["resumable"], true,
                "backend history is unchanged"
            );
            let output = super::super::result_to_tool_result(
                "branch-only",
                result,
                finalized.task_id,
                finalized.note,
            );
            assert!(output.output.contains("branch retained"));
            assert!(output.output.contains("cannot be resumed"));
            assert!(!output.output.contains("[task_id:"));
            assert!(!workspaces().lock().unwrap().contains_key(&task_id));
            assert!(!info.path.exists());
            git(repo.path(), &["rev-parse", "--verify", &info.branch]);
        }
    })
    .await
    .expect("branch-only finalization must settle without a stale resume");
}
