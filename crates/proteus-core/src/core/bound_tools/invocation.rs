//! Invocation timing, cancellation settlement, and bound extension callbacks.
use super::*;
use std::{
    future,
    time::{Duration, Instant},
};
use tokio::time::{sleep, timeout};
const CANCELLATION_SETTLEMENT_GRACE: Duration = Duration::from_millis(500);
impl BoundTools {
    pub(super) async fn invoke_allowed<F>(
        &self,
        observer: &dyn ToolExecutionObserver,
        call: &ToolCall,
        tool_spec: Option<ToolSpec>,
        cwd: PathBuf,
        enrich: F,
    ) -> Result<ToolResult>
    where
        F: FnOnce(&mut ToolContext),
    {
        observer.tool_call_resolved(call).await?;
        let tool = self
            .registry
            .get(&call.name)
            .ok_or_else(|| anyhow!("unknown tool: {}", call.name))?;
        let timeout_ms = tool_spec
            .as_ref()
            .and_then(|spec| spec.timeout_ms)
            .unwrap_or(self.default_timeout_ms);
        let started = Instant::now();
        let parent_cancellation =
            crate::core::model_call_scope::operation_cancellation(&self.binding.scope.cancellation);
        let tool_cancellation = parent_cancellation.child_token();
        let mut tool_ctx = ToolContext {
            cwd: cwd.clone(),
            attribution: self.binding.attribution,
            cancellation: tool_cancellation.clone(),
            user_input: None,
            task: None,
            agent_control: None,
            conversation: self.conversation.clone(),
            conversation_session_id: self.conversation_session_id,
            host: None,
        };
        enrich(&mut tool_ctx);
        // Binding-owned fields remain authoritative after optional enrichment.
        tool_ctx.attribution = self.binding.attribution;
        tool_ctx.cancellation = tool_cancellation.clone();
        tool_ctx.host = Some(Arc::new(
            crate::core::extension_operations::DelegatingToolHost {
                delegate: self.operations.clone(),
                tools: self.clone().with_conversation(
                    tool_ctx.conversation.clone(),
                    tool_ctx.conversation_session_id,
                ),
                cwd: cwd.clone(),
                owner: crate::contracts::OperationOwner::Tool {
                    call_id: call.id.clone(),
                },
                lineage: crate::core::model_call_scope::current_tool_lineage(),
                cancellation: tool_cancellation.clone(),
            },
        ));
        let timeout_future = async move {
            if timeout_ms == 0 {
                future::pending::<()>().await;
            } else {
                sleep(Duration::from_millis(timeout_ms)).await;
            }
        };
        tokio::pin!(timeout_future);
        let invocation = tool.invoke(call, tool_ctx);
        tokio::pin!(invocation);
        enum InvocationOutcome<T> {
            Completed(T),
            TimedOut,
            Canceled,
        }
        let outcome = tokio::select! {
            result = &mut invocation => InvocationOutcome::Completed(result),
            _ = &mut timeout_future => InvocationOutcome::TimedOut,
            _ = parent_cancellation.cancelled() => InvocationOutcome::Canceled,
        };
        let result = match outcome {
            InvocationOutcome::Completed(result) => match result {
                Ok(result) => result,
                Err(error) => ToolResult::error(call.id.clone(), error.to_string())
                    .with_metadata(json!({ "tool": call.name })),
            },
            InvocationOutcome::TimedOut => {
                tool_cancellation.cancel();
                // Keep polling process-backed tools long enough for the
                // invocation handle to forward protocol cancel and settle.
                // A tool that ignores cancellation is still bounded here.
                let _ = timeout(CANCELLATION_SETTLEMENT_GRACE, &mut invocation).await;
                ToolResult::error(
                    call.id.clone(),
                    format!("tool timed out after {timeout_ms}ms"),
                )
                .with_metadata(json!({
                    "tool": call.name,
                    "timed_out": true,
                    "timeout_ms": timeout_ms,
                }))
            }
            InvocationOutcome::Canceled => {
                tool_cancellation.cancel();
                let _ = timeout(CANCELLATION_SETTLEMENT_GRACE, &mut invocation).await;
                ToolResult::error(call.id.clone(), "tool call canceled").with_metadata(json!({
                    "tool": call.name,
                    "canceled": true,
                }))
            }
        };

        let mut result = self.truncate_result(result);
        result.metadata = metadata_with(
            result.metadata,
            "duration_ms",
            json!(started.elapsed().as_millis() as u64),
        );
        self.finish(observer, &cwd, call, result).await
    }
}
