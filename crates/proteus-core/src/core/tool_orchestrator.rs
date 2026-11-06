use std::{path::Path, sync::Arc};

use anyhow::Result;

use crate::{
    contracts::{AgentWorkflowContext, ExecutionAttribution, RequestOrigin, ToolContext},
    core::{AttributedUserInputTransport, BoundTools, ToolExecutionBinding},
    domain::{AgentTask, ToolCall, ToolResult, ToolSpec},
};

const DEFAULT_MAX_OUTPUT_BYTES: usize = 200_000;

/// Agent-layer adapter around the generic execution-bound tool mechanism.
#[derive(Debug, Clone)]
pub struct ToolOrchestrator {
    default_timeout_ms: u64,
    max_output_bytes: usize,
}

impl Default for ToolOrchestrator {
    fn default() -> Self {
        Self {
            default_timeout_ms: 30_000,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

impl ToolOrchestrator {
    pub fn visible_tool_specs(&self, ctx: &AgentWorkflowContext, cwd: &Path) -> Vec<ToolSpec> {
        self.bind(ctx).visible_specs(cwd)
    }

    pub async fn execute(
        &self,
        ctx: &AgentWorkflowContext,
        task: &AgentTask,
        call: ToolCall,
    ) -> Result<ToolResult> {
        if ctx.is_cancelled() {
            anyhow::bail!("turn canceled by client");
        }

        let bound = self.bind(ctx);
        let origin = request_origin(ctx);
        bound
            .execute_enriched(task.cwd.clone(), call, bound.observer(), |tool_ctx| {
                enrich_agent_tool_context(ctx, task, origin, tool_ctx);
            })
            .await
    }

    fn bind(&self, ctx: &AgentWorkflowContext) -> BoundTools {
        let binding = ToolExecutionBinding::attributed(
            ctx.execution.scope.clone(),
            tool_attribution(ctx),
            request_origin(ctx),
            ctx.tool_recorder.clone(),
        )
        .with_terminal_factory(ctx.terminal_registry.as_ref().map(|registry| {
            super::tool_terminal_binding::for_turn(
                registry.clone(),
                ctx.events.clone(),
                ctx.event_context(),
            )
        }))
        .with_observer(Some(super::tool_terminal_binding::observer_for_turn(
            ctx.events.clone(),
            ctx.event_context(),
        )));
        BoundTools::new(
            ctx.execution.tools.clone(),
            ctx.execution.policy.clone(),
            ctx.execution.approval.clone(),
            ctx.execution.permission_grants.clone(),
            binding,
        )
        .with_limits(self.default_timeout_ms, self.max_output_bytes)
        .with_hooks(ctx.execution.hooks.clone())
        .with_operations(ctx.execution.operations.clone())
    }
}

fn enrich_agent_tool_context(
    ctx: &AgentWorkflowContext,
    task: &AgentTask,
    origin: RequestOrigin,
    tool_ctx: &mut ToolContext,
) {
    tool_ctx.user_input = Some(Arc::new(AttributedUserInputTransport::new(
        ctx.user_input.clone(),
        origin,
    )));
    tool_ctx.task = Some(task.clone());
    tool_ctx.conversation = ctx.conversation.clone();
    tool_ctx.conversation_session_id = Some(ctx.session_id);
    tool_ctx.agent_control =
        crate::core::agent_control::bind_tool_host(ctx, tool_ctx.cancellation.clone());
}

fn request_origin(ctx: &AgentWorkflowContext) -> RequestOrigin {
    let origin =
        RequestOrigin::for_turn(ctx.execution.scope.execution_id, ctx.thread_id, ctx.turn_id);
    match &ctx.thread_label {
        Some(label) => origin.with_label(label.clone()),
        None => origin,
    }
}

fn tool_attribution(ctx: &AgentWorkflowContext) -> ExecutionAttribution {
    ExecutionAttribution::for_turn(
        ctx.execution.scope.execution_id,
        ctx.session_id,
        ctx.thread_id,
        ctx.turn_id,
    )
}
