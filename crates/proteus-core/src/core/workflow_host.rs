//! Core-owned implementation of the Workflow host capability surface.
//!
//! Process Workflow delegates here, so module identity cannot change model,
//! tool, policy, cancellation, or event semantics.

mod model_stream;

use std::{future::Future, path::PathBuf};

use anyhow::{Result, anyhow};

use crate::{
    contracts::{
        ContextBuildInput, ContextPreparationInput, ContextPreparationResult, ExecutionAttribution,
        HookEvent, HookInput, ModelCallOrigin, ToolSelectionInput, ToolSelectionOutput,
        ToolSelectionRequest, WorkflowInvocationContext, WorkflowRuntimeStatus,
    },
    domain::{AgentTask, Event, ToolCall, ToolResult, ToolSpec},
    model_standard::{CanonicalModelRequest, CanonicalModelResponse},
};

use super::{
    ToolOrchestrator,
    agent_control::{TASK_TOOL, calls_are_parallel_eligible},
    model_call_scope::with_model_call_origin,
};

/// Async host capability surface shared by all process Workflow exports.
pub(crate) struct WorkflowHostRuntime {
    ctx: WorkflowInvocationContext,
    cwd: PathBuf,
    tool_orchestrator: ToolOrchestrator,
    model_streams: model_stream::ModelStreams,
    operation_gate: tokio::sync::RwLock<()>,
    rejected_preparation: std::sync::atomic::AtomicBool,
}

impl WorkflowHostRuntime {
    pub(crate) async fn checkpoint_history(
        &self,
        checkpoint: crate::contracts::WorkflowHistoryCheckpoint,
    ) -> Result<crate::contracts::WorkflowCheckpointAck> {
        self.ensure_active()?;
        let preparation = checkpoint.preparation_target.is_some();
        let result = self
            .ctx
            .agent()?
            .history_recorder
            .checkpoint(checkpoint)
            .await;
        if preparation {
            self.rejected_preparation
                .store(result.is_err(), std::sync::atomic::Ordering::SeqCst);
        }
        result
    }
    pub(crate) fn has_rejected_preparation(&self) -> bool {
        self.rejected_preparation
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn new(ctx: WorkflowInvocationContext, cwd: PathBuf) -> Self {
        Self {
            ctx,
            cwd,
            model_streams: Default::default(),
            operation_gate: Default::default(),
            rejected_preparation: Default::default(),
            tool_orchestrator: ToolOrchestrator::default(),
        }
    }

    pub(crate) fn status(&self) -> WorkflowRuntimeStatus {
        WorkflowRuntimeStatus {
            cancelled: self.ctx.execution().is_cancelled(),
            queued_user_messages: self
                .ctx
                .agent()
                .map(|ctx| ctx.queued_user_messages())
                .unwrap_or(0)
                .min(u32::MAX as usize) as u32,
        }
    }

    pub(crate) async fn build_context(
        &self,
        task: AgentTask,
    ) -> Result<crate::domain::ContextBundle> {
        let ctx = &self.ctx;
        let execution = ctx.execution();
        let attribution = match ctx.agent() {
            Ok(agent) => ExecutionAttribution::for_turn(
                execution.scope.execution_id,
                agent.session_id,
                agent.thread_id,
                agent.turn_id,
            ),
            Err(_) => ExecutionAttribution::detached(execution.scope.execution_id),
        };
        ctx.context()
            .build(ContextBuildInput {
                task,
                scope: execution.scope.clone(),
                attribution,
            })
            .await
    }

    pub(crate) async fn start_model_stream(
        &self,
        request: CanonicalModelRequest,
    ) -> Result<crate::contracts::WorkflowModelStreamCursor> {
        anyhow::ensure!(
            !self.has_rejected_preparation(),
            "model sampling requires an accepted context preparation checkpoint"
        );
        let _guard = self.operation_gate.read().await;
        self.ensure_active()?;
        self.model_streams
            .start(self.ctx.execution(), request)
            .await
    }

    pub(crate) async fn next_model_stream(
        &self,
        cursor: crate::contracts::WorkflowModelStreamCursor,
    ) -> Result<crate::contracts::WorkflowModelStreamItem> {
        self.run_active(self.model_streams.next(cursor)).await
    }

    pub(crate) async fn close_model_stream(&self) -> bool {
        self.model_streams.close().await
    }

    pub(crate) async fn complete_model(
        &self,
        request: CanonicalModelRequest,
    ) -> Result<CanonicalModelResponse> {
        anyhow::ensure!(
            !self.has_rejected_preparation(),
            "model sampling requires an accepted context preparation checkpoint"
        );
        let _guard = self.operation_gate.read().await;
        let ctx = self.ctx.clone();
        self.run_active(async move {
            with_model_call_origin(
                ModelCallOrigin::Direct,
                ctx.execution().require_model()?.complete(request),
            )
            .await
        })
        .await
    }

    pub(crate) async fn prepare_context(
        &self,
        input: ContextPreparationInput,
    ) -> Result<ContextPreparationResult> {
        let _guard = self.operation_gate.try_write().map_err(|_| {
            anyhow!("context preparation requires completed model and tool operations")
        })?;
        anyhow::ensure!(
            !self.model_streams.is_active().await,
            "context preparation requires a closed model stream"
        );
        let execution = self.ctx.execution();
        let attribution = self
            .ctx
            .agent()
            .map(|ctx| {
                ExecutionAttribution::for_turn(
                    ctx.execution.scope.execution_id,
                    ctx.session_id,
                    ctx.thread_id,
                    ctx.turn_id,
                )
            })
            .unwrap_or_else(|_| ExecutionAttribution::detached(execution.scope.execution_id));
        let preparation: std::sync::Arc<dyn crate::contracts::ContextPreparation> =
            match self.ctx.agent() {
                Ok(ctx) => ctx.context_preparation.clone(),
                Err(_) => {
                    std::sync::Arc::new(super::context_preparation::RuntimeContextPreparation::new(
                        execution.clone(),
                        attribution,
                        self.cwd.clone(),
                        None,
                    ))
                }
            };
        self.run_active(async {
            if let Ok(ctx) = self.ctx.agent() {
                ctx.emit(Event::HistoryCompactionStarted { reason: input.reason.clone(), input_messages: input.messages.len(), token_estimate: input.token_estimate, trigger_tokens: None }).await?;
            }
            let result = preparation.prepare(input.clone()).await;
            if let Ok(ctx) = self.ctx.agent() {
                match &result {
                    Ok(result) => ctx.emit(Event::HistoryCompactionCompleted { report: crate::domain::HistoryCompactionReport::from_context_preparation_output(&input, &result.output) }).await?,
                    Err(error) => ctx.emit(Event::HistoryCompactionFailed { reason: input.reason.clone(), input_messages: input.messages.len(), token_estimate: input.token_estimate, trigger_tokens: None, message: format!("{error:#}") }).await?,
                }
            }
            result
        }).await
    }

    pub(crate) fn visible_tools(&self, cwd: PathBuf) -> Result<Vec<ToolSpec>> {
        self.ensure_active()?;
        match self.ctx.agent() {
            Ok(ctx) => Ok(self.tool_orchestrator.visible_tool_specs(ctx, &cwd)),
            Err(_) => Ok(self.detached_tools().visible_specs(&cwd)),
        }
    }

    pub(crate) async fn select_tools(
        &self,
        request: ToolSelectionRequest,
    ) -> Result<ToolSelectionOutput> {
        let candidates = self.visible_tools(request.cwd.clone())?;
        let input = ToolSelectionInput::new(request, candidates.clone());
        let execution = self.ctx.execution();
        let attribution = self
            .ctx
            .agent()
            .map(|agent| {
                ExecutionAttribution::for_turn(
                    execution.scope.execution_id,
                    agent.session_id,
                    agent.thread_id,
                    agent.turn_id,
                )
            })
            .unwrap_or_else(|_| ExecutionAttribution::detached(execution.scope.execution_id));
        self.run_active(async {
            let conversation = match self
                .ctx
                .agent()
                .ok()
                .and_then(|agent| agent.conversation.as_ref())
            {
                Some(reader) => Some(reader.read().await?),
                None => None,
            };
            let accepted = execution
                .hooks
                .apply(HookInput {
                    event: HookEvent::BeforeToolSelection {
                        input: input.clone(),
                        output: ToolSelectionOutput::new(candidates.clone()),
                    },
                    attribution,
                    cwd: input.request.cwd.clone(),
                    conversation,
                })
                .await?;
            let HookEvent::BeforeToolSelection {
                input: accepted_input,
                output,
            } = accepted
            else {
                anyhow::bail!("tool selection hook returned a different event");
            };
            anyhow::ensure!(
                accepted_input == input,
                "tool selection changed bound input"
            );
            output.validate_against(&candidates)?;
            Ok(output)
        })
        .await
    }

    pub(crate) async fn execute_tool(&self, task: AgentTask, call: ToolCall) -> Result<ToolResult> {
        let _guard = self.operation_gate.read().await;
        self.run_active(async {
            match self.ctx.agent() {
                Ok(ctx) => self.tool_orchestrator.execute(ctx, &task, call).await,
                Err(_) => self.detached_tools().execute(task.cwd, call).await,
            }
        })
        .await
    }

    pub(crate) async fn execute_tools(
        &self,
        task: AgentTask,
        calls: Vec<ToolCall>,
    ) -> Result<Vec<ToolResult>> {
        let _guard = self.operation_gate.read().await;
        self.run_active(execute_tool_batch(self, &task, calls))
            .await
    }

    pub(crate) async fn emit_event(&self, event: Event) -> Result<()> {
        let ctx = self.ctx.agent()?.clone();
        self.run_active(async move { ctx.emit(event).await }).await
    }

    fn detached_tools(&self) -> super::BoundTools {
        let ctx = self.ctx.execution();
        super::BoundTools::new(
            ctx.tools.clone(),
            ctx.policy.clone(),
            ctx.approval.clone(),
            ctx.permission_grants.clone(),
            super::ToolExecutionBinding::detached(ctx.scope.clone()),
        )
        .with_hooks(ctx.hooks.clone())
        .with_operations(ctx.operations.clone())
    }

    fn ensure_active(&self) -> Result<()> {
        if self.ctx.execution().is_cancelled() {
            return Err(anyhow!("turn canceled by client"));
        }
        Ok(())
    }

    async fn run_active<T>(&self, future: impl Future<Output = Result<T>>) -> Result<T> {
        let cancellation = self.ctx.execution().scope.cancellation.clone();
        if cancellation.is_cancelled() {
            return Err(anyhow!("turn canceled by client"));
        }
        tokio::select! {
            result = future => result,
            _ = cancellation.cancelled() => Err(anyhow!("turn canceled by client")),
        }
    }
}

/// Executes a batch through the same registry/policy/safety path as in-process
/// workflows. Consecutive explicitly parallel calls may overlap; other calls
/// fence the sequence. The root-owned task group has its own role eligibility.
async fn execute_tool_batch(
    host: &WorkflowHostRuntime,
    task: &AgentTask,
    calls: Vec<ToolCall>,
) -> Result<Vec<ToolResult>> {
    let specs = host.visible_tools(task.cwd.clone())?;
    let parallel = |call: &ToolCall| {
        specs
            .iter()
            .find(|spec| spec.name == call.name)
            .is_some_and(|spec| spec.supports_parallel_tool_calls)
    };

    let mut results = Vec::with_capacity(calls.len());
    let mut queue = calls.into_iter().peekable();
    while let Some(call) = queue.next() {
        if call.name == TASK_TOOL {
            let mut group = vec![call];
            while queue.peek().is_some_and(|call| call.name == TASK_TOOL) {
                group.push(queue.next().expect("peeked task call"));
            }
            if calls_are_parallel_eligible(
                &group,
                &host
                    .ctx
                    .agent()
                    .ok()
                    .and_then(|ctx| ctx.agent_control.as_ref())
                    .map(|control| control.profiles())
                    .unwrap_or_default(),
            ) {
                let outputs = futures_util::future::join_all(
                    group
                        .into_iter()
                        .map(|call| host.execute_tool(task.clone(), call)),
                )
                .await;
                for output in outputs {
                    results.push(output?);
                }
            } else {
                for call in group {
                    results.push(host.execute_tool(task.clone(), call).await?);
                }
            }
            continue;
        }
        if parallel(&call) {
            let mut group = vec![call];
            while queue.peek().is_some_and(&parallel) {
                group.push(queue.next().expect("peeked call"));
            }
            let outputs = futures_util::future::join_all(
                group
                    .into_iter()
                    .map(|call| host.execute_tool(task.clone(), call)),
            )
            .await;
            for output in outputs {
                results.push(output?);
            }
        } else {
            results.push(host.execute_tool(task.clone(), call).await?);
        }
    }
    Ok(results)
}

#[cfg(test)]
#[path = "workflow_host/selection_tests.rs"]
mod selection_tests;
