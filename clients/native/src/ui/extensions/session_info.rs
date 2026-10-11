//! Session facts shared by the built-in icons.

use proteus_contracts::app_protocol::AppSessionActivityStatus;
use proteus_contracts::app_protocol::config::ConfigSummary;

use super::host::HostView;
use crate::tool_display::{self, PlanStep, StepStatus};
use crate::transcript::Role;
use crate::ui::chat::tool_row::tool_args;

pub(super) const CONTEXT: &str = "context";
pub(super) const PLAN: &str = "plan";
pub(super) const SESSION: &str = "session";
pub(super) const AGENT: &str = "agent";

/// Steps of the latest `update_plan` call of the root thread.
pub(super) fn current_plan(host: &HostView) -> Vec<PlanStep> {
    host.session()
        .and_then(|session| {
            session.transcript.entries().iter().rev().find_map(|entry| {
                entry
                    .tool
                    .as_ref()
                    .filter(|tool| tool.name == tool_display::UPDATE_PLAN)
            })
        })
        .map(|tool| tool_display::plan_steps(tool_args(tool)))
        .unwrap_or_default()
}

pub(super) fn completed(steps: &[PlanStep]) -> usize {
    steps
        .iter()
        .filter(|step| step.status == StepStatus::Completed)
        .count()
}

pub(super) struct SessionStats {
    pub status: &'static str,
    pub attention: bool,
    pub messages: usize,
    pub tools: usize,
    pub waiting: usize,
    pub queued: usize,
}

pub(super) fn session_stats(host: &HostView) -> Option<SessionStats> {
    let session = host.session()?;
    let activity = host.activity().map(|activity| &activity.status);
    let status = match (session.connected, activity) {
        (false, _) => "Нет связи",
        (true, Some(AppSessionActivityStatus::Running)) => "Работает",
        (true, Some(AppSessionActivityStatus::WaitingApproval)) => "Ждёт подтверждения",
        (true, Some(AppSessionActivityStatus::WaitingInput)) => "Ждёт ответа",
        (true, _) if session.running() => "Работает",
        (true, _) => "Ожидает",
    };
    let entries = session.transcript.entries();
    let messages = entries
        .iter()
        .filter(|entry| entry.tool.is_none() && entry.subagent.is_none())
        .filter(|entry| matches!(entry.role, Role::User | Role::Assistant))
        .count();
    let tools = entries
        .iter()
        .map(|entry| {
            usize::from(entry.tool.is_some())
                + entry.subagent.as_ref().map_or(0, |agent| agent.tools.len())
        })
        .sum();
    let (waiting, queued) = session.pending.as_ref().map_or((0, 0), |pending| {
        (
            pending.approvals.len() + pending.user_inputs.len(),
            pending.queued_user_messages.len(),
        )
    });
    Some(SessionStats {
        status,
        attention: !session.connected
            || matches!(
                activity,
                Some(
                    AppSessionActivityStatus::WaitingApproval
                        | AppSessionActivityStatus::WaitingInput
                )
            ),
        messages,
        tools,
        waiting,
        queued,
    })
}

pub(super) fn model_label(config: &ConfigSummary) -> String {
    config.model.as_ref().map_or("—".to_owned(), |model| {
        if model.label.is_empty() {
            model.name.clone()
        } else {
            model.label.clone()
        }
    })
}
