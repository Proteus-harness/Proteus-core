use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use anyhow::{Result, anyhow};
use serde_json::Value;

use crate::{
    contracts::{
        AgentControlRequest, AgentControlResult, AgentControlToolHost, AgentIsolation,
        AgentLifecycleStatus, AgentProfile, CancellationToken, WorkspaceInfo,
    },
    domain::{SessionId, ToolResult},
};

use super::super::workspace as git_workspace;
use git_workspace::CleanupOutcome;

enum CleanupPolicy {
    // No child received this fresh checkout; task cancellation must not leak it.
    UnstartedPreparation,
    CompletedTurn(CancellationToken),
}

#[derive(Default)]
struct FinalizedWorkspace {
    note: Option<String>,
    // A backend-retained history is insufficient without this facade's map.
    task_id: Option<String>,
}

pub(super) struct PreparedWorkspace {
    info: WorkspaceInfo,
    resume_task_id: Option<String>,
    session_id: Option<SessionId>,
}

struct WorkspaceRecord {
    // None means an admitted task owns this worktree through its terminal decision.
    info: Option<WorkspaceInfo>,
    session_id: Option<SessionId>,
}

/// The foreground waiter may disappear during blocking preparation or child
/// cancellation grace. One detached owner retains the workspace until its
/// terminal decision; Drop only cancels the tool invocation, never the directory.
pub(super) async fn run_owned(
    call_id: String,
    mut request: AgentControlRequest,
    role: AgentProfile,
    host: Arc<dyn AgentControlToolHost>,
    cancellation: CancellationToken,
) -> Result<ToolResult> {
    let mut waiter = WaiterCancellation(Some(cancellation.clone()));
    let completion = tokio::spawn(async move {
        if cancellation.is_cancelled() {
            return super::task_error(&call_id, "task canceled before worktree preparation".into());
        }
        let workspace =
            match prepare_workspace(&mut request, &call_id, &role, host.session_id()).await {
                Ok(workspace) => workspace,
                Err(message) => return super::task_error(&call_id, message),
            };
        // Do not dispatch if cancellation is already visible after preparation.
        if cancellation.is_cancelled() {
            let workspace = restore_unstarted_resume(workspace);
            return error_result_with_workspace(
                &call_id,
                "task canceled before child dispatch".into(),
                workspace,
                Some(CleanupPolicy::UnstartedPreparation),
            )
            .await;
        }
        match host.run_agent(request).await {
            Ok(result) => {
                // A canceled child turn can finish before its process-tool
                // workers settle. Keep the checkout when cancellation is
                // visible here or the child did not complete successfully.
                // The helper rechecks after inspection before each mutation;
                // it cannot stop one already dispatched or prove quiescence.
                let allow_cleanup = !cancellation.is_cancelled()
                    && result.status == AgentLifecycleStatus::Completed;
                let finalized = finalize_workspace(
                    workspace,
                    &result,
                    allow_cleanup.then(|| CleanupPolicy::CompletedTurn(cancellation.clone())),
                )
                .await;
                super::result_to_tool_result(&call_id, result, finalized.task_id, finalized.note)
            }
            Err(error) => {
                // Err does not distinguish pre-dispatch rollback from a
                // failed child with effects still settling. Preserve the path.
                error_result_with_workspace(&call_id, format!("{error:#}"), workspace, None).await
            }
        }
    });
    let result = completion
        .await
        .map_err(|error| anyhow!("worktree task owner failed: {error}"))?;
    waiter.0 = None;
    Ok(result)
}

struct WaiterCancellation(Option<CancellationToken>);

impl Drop for WaiterCancellation {
    fn drop(&mut self) {
        if let Some(cancellation) = self.0.take() {
            cancellation.cancel();
        }
    }
}

// No child was dispatched: return an existing checkout to the idle registry,
// rather than treating cancellation as a terminal child that may be cleaned.
// A freshly created checkout still belongs to the ordinary cleanup path.
fn restore_unstarted_resume(workspace: Option<PreparedWorkspace>) -> Option<PreparedWorkspace> {
    let workspace = workspace?;
    let Some(task_id) = workspace.resume_task_id.as_deref() else {
        return Some(workspace);
    };
    if let Ok(mut map) = workspaces().lock()
        && let Some(record) = map.get_mut(task_id)
        && record.session_id == workspace.session_id
        && record.info.is_none()
    {
        record.info = Some(workspace.info);
    }
    None
}

pub(super) async fn prepare_workspace(
    request: &mut AgentControlRequest,
    call_id: &str,
    role: &AgentProfile,
    session_id: Option<SessionId>,
) -> Result<Option<PreparedWorkspace>, String> {
    if role.isolation != AgentIsolation::Worktree {
        return Ok(None);
    }
    let resume_task_id = request
        .metadata
        .get("task_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let info = match resume_task_id.as_deref() {
        Some(task_id) => {
            let mut map = workspaces().lock().ok();
            let record = map
                .as_mut()
                .and_then(|map| map.get_mut(task_id))
                .ok_or_else(|| {
                    format!(
                        "worktree for task_id {task_id} is gone (removed, merged, or lost on restart); start a fresh task instead"
                    )
                })?;
            if record.session_id != session_id {
                return Err(format!(
                    "worktree for task_id {task_id} is gone (removed, merged, or belongs to another session); start a fresh task instead"
                ));
            }
            record.info.take().ok_or_else(|| {
                format!("worktree for task_id {task_id} is already owned by an active task")
            })?
        }
        None => {
            let parent_cwd = request.task.cwd.clone();
            let name = workspace_name(&request.role, call_id);
            tokio::task::spawn_blocking(move || git_workspace::create_worktree(&parent_cwd, &name))
                .await
                .map_err(|error| format!("worktree worker failed: {error}"))?
                .map_err(|error| format!("{error:#}"))?
        }
    };
    request.task.cwd = info.path.clone();
    Ok(Some(PreparedWorkspace {
        info,
        resume_task_id,
        session_id,
    }))
}

async fn finalize_workspace(
    workspace: Option<PreparedWorkspace>,
    result: &AgentControlResult,
    cleanup_policy: Option<CleanupPolicy>,
) -> FinalizedWorkspace {
    let Some(workspace) = workspace else {
        return FinalizedWorkspace::default();
    };
    if let Some(task_id) = workspace.resume_task_id.as_deref() {
        remove_workspace(task_id, workspace.session_id);
    }
    let info = workspace.info;
    let cleanup = if let Some(policy) = cleanup_policy {
        cleanup_workspace(info.clone(), policy).await
    } else {
        Ok((info.clone(), retained_outcome(&info)))
    };
    let mut resumable_task_id = None;
    let note = match cleanup {
        Ok((_, CleanupOutcome::Removed)) => {
            if let Some(task_id) = child_task_id(result) {
                remove_workspace(&task_id, workspace.session_id);
            }
            "[worktree: no changes, removed]".to_owned()
        }
        Ok((info, CleanupOutcome::Retained)) => {
            if let Some(task_id) = child_task_id(result)
                && let Ok(mut map) = workspaces().lock()
            {
                map.insert(
                    task_id.clone(),
                    WorkspaceRecord {
                        info: Some(info.clone()),
                        session_id: workspace.session_id,
                    },
                );
                resumable_task_id = Some(task_id);
            }
            format!(
                "[worktree: {} | branch: {}]\nThe subagent's changes live only on that branch. Review and merge it yourself, then remove the worktree.",
                info.path.display(),
                info.branch
            )
        }
        Ok((info, CleanupOutcome::BranchRetained)) => {
            if let Some(task_id) = child_task_id(result) {
                remove_workspace(&task_id, workspace.session_id);
            }
            branch_retained_note(&info)
        }
        Err(error) => format!(
            "[worktree cleanup failed: {error:#}; inspect checkout {} and branch {}]",
            info.path.display(),
            info.branch
        ),
    };
    FinalizedWorkspace {
        note: Some(note),
        task_id: resumable_task_id,
    }
}

async fn error_result_with_workspace(
    call_id: &str,
    mut message: String,
    workspace: Option<PreparedWorkspace>,
    cleanup_policy: Option<CleanupPolicy>,
) -> ToolResult {
    if let Some(workspace) = workspace {
        if let Some(task_id) = workspace.resume_task_id.as_deref() {
            remove_workspace(task_id, workspace.session_id);
        }
        let info = workspace.info;
        let cleanup = if let Some(policy) = cleanup_policy {
            cleanup_workspace(info.clone(), policy).await
        } else {
            Ok((info.clone(), retained_outcome(&info)))
        };
        match cleanup {
            Ok((_, CleanupOutcome::Removed)) => {}
            Ok((info, CleanupOutcome::Retained)) => message.push_str(&format!(
                "\n[worktree left at {} | branch: {}]",
                info.path.display(),
                info.branch
            )),
            Ok((info, CleanupOutcome::BranchRetained)) => {
                message.push('\n');
                message.push_str(&branch_retained_note(&info));
            }
            Err(_) => message.push_str(&format!(
                "\n[worktree cleanup incomplete; inspect checkout {} and branch {}]",
                info.path.display(),
                info.branch
            )),
        }
    }
    super::task_error(call_id, message)
}

#[cfg(test)]
mod tests;

fn remove_workspace(task_id: &str, session_id: Option<SessionId>) {
    if let Ok(mut map) = workspaces().lock()
        && map
            .get(task_id)
            .is_some_and(|record| record.session_id == session_id)
    {
        map.remove(task_id);
    }
}

fn workspaces() -> &'static Mutex<HashMap<String, WorkspaceRecord>> {
    static WORKSPACES: OnceLock<Mutex<HashMap<String, WorkspaceRecord>>> = OnceLock::new();
    WORKSPACES.get_or_init(Mutex::default)
}

/*
 * Keep the remaining helpers below focused on git worktree mechanics and
 * resumable-result interpretation.
 */

fn workspace_name(role: &str, call_id: &str) -> String {
    let sanitize = |value: &str| {
        value
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>()
    };
    format!("{}-{}", sanitize(role), sanitize(call_id))
        .trim_start_matches(['.', '-'])
        .to_owned()
}

pub(super) fn child_task_id(result: &AgentControlResult) -> Option<String> {
    result
        .metadata
        .get("resumable")
        .and_then(Value::as_bool)
        .filter(|resumable| *resumable)
        .and_then(|_| result.child_thread_id.map(|id| id.to_string()))
}

fn retained_outcome(info: &WorkspaceInfo) -> CleanupOutcome {
    if info.path.exists() {
        CleanupOutcome::Retained
    } else {
        CleanupOutcome::BranchRetained
    }
}

fn branch_retained_note(info: &WorkspaceInfo) -> String {
    format!(
        "[worktree checkout absent: {} | branch retained: {}]\nReview the branch and worktree bookkeeping manually; this checkout cannot be resumed.",
        info.path.display(),
        info.branch
    )
}

async fn cleanup_workspace(
    info: WorkspaceInfo,
    policy: CleanupPolicy,
) -> Result<(WorkspaceInfo, CleanupOutcome)> {
    tokio::task::spawn_blocking(move || {
        let outcome = git_workspace::cleanup_worktree_if_unchanged(&info, || match &policy {
            CleanupPolicy::UnstartedPreparation => false,
            CleanupPolicy::CompletedTurn(cancellation) => cancellation.is_cancelled(),
        })?;
        Ok((info, outcome))
    })
    .await
    .map_err(|error| anyhow!("worktree cleanup worker failed: {error}"))?
}
