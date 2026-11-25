//! Root-owned agent-control facade and its process lifecycle implementation.
//!
//! A child is always another complete Proteus process with its own config,
//! runtime, model, tools and policy. External core consumers see only
//! [`AgentControlRuntime`]: process connection, pool, mailbox, pending state,
//! per-invocation binding and model-facing tool facades remain private here.

mod collaboration;
mod history;
mod mailbox;
mod model_options;
mod pending;
mod process;
mod task;
mod tool_host;
mod workspace;

use std::sync::Arc;

use anyhow::{Result, bail};
use serde_json::Value;

use crate::contracts::{
    AgentAddress, AgentControl, AgentControlRequest, AgentWorkflowContext, CancellationToken,
    ToolRegistry, ToolSource,
};

use super::{AgentControlConfig, AgentControlSurface};
use history::HistoryNotifier;
use process::ProcessAgentControl;

pub(crate) use task::{TASK_TOOL, calls_are_parallel_eligible};

const TASK_SOURCE: &str = "agent-control-task";
const COLLABORATION_SOURCE: &str = "agent-control-collaboration";

/// Keep the UTF-8 prefix without retaining a larger original allocation.
fn retain_summary(text: String, max_bytes: usize) -> String {
    if text.capacity() <= max_bytes {
        return text;
    }
    text[..text.floor_char_boundary(max_bytes)].to_owned()
}

pub(crate) fn owns_tool_source(source: &ToolSource) -> bool {
    matches!(source, ToolSource::Builtin { provider }
        if provider == TASK_SOURCE || provider == COLLABORATION_SOURCE)
}

/// Единственная core-facing точка сборки process agent control и его
/// model-facing facade. Внешние слои не знают process pool, mailbox или
/// child-config representation.
#[derive(Clone)]
pub struct AgentControlRuntime {
    service: Option<Arc<dyn AgentControl>>,
    surface: AgentControlSurface,
}

impl AgentControlRuntime {
    pub fn from_config(config: &AgentControlConfig) -> Result<Self> {
        let service = if config.roles.is_empty() {
            None
        } else {
            let history_notifier = HistoryNotifier::new(collaboration::bind_history);
            Some(Arc::new(ProcessAgentControl::from_config(
                config.clone(),
                history_notifier,
            )?) as Arc<dyn AgentControl>)
        };
        Ok(Self {
            service,
            surface: config.surface,
        })
    }

    pub fn disabled() -> Self {
        Self {
            service: None,
            surface: AgentControlSurface::None,
        }
    }

    pub fn service(&self) -> Option<Arc<dyn AgentControl>> {
        self.service.clone()
    }

    /// Регистрирует ровно одну configured facade поверх того же service,
    /// который будет помещён в `AgentWorkflowContext`.
    pub fn register_tools(&self, tools: &mut ToolRegistry, timeout_ms: u64) -> Result<()> {
        let Some(service) = self.service.as_ref() else {
            return Ok(());
        };
        match self.surface {
            AgentControlSurface::Task => {
                task::register_task_tool(tools, service.profiles(), timeout_ms)
            }
            AgentControlSurface::Collaboration => {
                collaboration::register_collaboration_tools(tools, service.profiles(), timeout_ms)
            }
            AgentControlSurface::None => Ok(()),
        }
    }
}

pub(crate) fn bind_tool_host(
    ctx: &AgentWorkflowContext,
    cancellation: CancellationToken,
) -> Option<Arc<dyn crate::contracts::AgentControlToolHost>> {
    tool_host::bind(ctx, cancellation)
}

fn requested_agent_target(request: &AgentControlRequest) -> Result<Option<AgentAddress>> {
    let control_plane_owned = request
        .metadata
        .get("control_plane_owned")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let target = request
        .metadata
        .get("agent_control_target")
        .and_then(Value::as_str);
    match (control_plane_owned, target) {
        (true, Some(target)) => AgentAddress::parse(target).map(Some),
        (true, None) => bail!("collaboration-owned agent requires agent_control_target"),
        (false, Some(_)) => bail!("agent_control_target requires control_plane_owned=true"),
        (false, None) => Ok(None),
    }
}
