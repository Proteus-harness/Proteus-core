use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{
    contracts::{
        ApprovalPolicy, ApprovalRequest, ApprovalTransport, ExecutionAttribution,
        ExecutionPermissionGrants, ExecutionScope, NoopToolExecutionRecorder, PolicyContext,
        PolicyVisibilityContext, RequestOrigin, ToolContext, ToolExecutionRecorder, ToolRegistry,
    },
    domain::{
        PolicyDecision, ToolCall, ToolCallResolution, ToolResult, ToolSpec, validate_tool_call_args,
    },
};

use self::support::{metadata_with, truncate_utf8, visibility_decision_allows};

mod hooks;
mod invocation;
mod support;

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 200_000;

/// Immutable execution binding for the tool capability.
///
/// Detached execution carries only its execution identity. An upper layer may
/// add an optional presentation projection without putting it into the scope.
#[derive(Clone)]
pub struct ToolExecutionBinding {
    scope: ExecutionScope,
    attribution: ExecutionAttribution,
    origin: RequestOrigin,
    recorder: Arc<dyn ToolExecutionRecorder>,
}

impl ToolExecutionBinding {
    pub fn detached(scope: ExecutionScope) -> Self {
        let execution_id = scope.execution_id;
        Self {
            scope,
            attribution: ExecutionAttribution::detached(execution_id),
            origin: RequestOrigin::for_execution(execution_id),
            recorder: Arc::new(NoopToolExecutionRecorder),
        }
    }

    pub fn with_recorder(mut self, recorder: Arc<dyn ToolExecutionRecorder>) -> Self {
        self.recorder = recorder;
        self
    }

    pub(crate) fn attributed(
        scope: ExecutionScope,
        attribution: ExecutionAttribution,
        origin: RequestOrigin,
        recorder: Arc<dyn ToolExecutionRecorder>,
    ) -> Self {
        debug_assert_eq!(scope.execution_id, attribution.execution_id);
        debug_assert_eq!(scope.execution_id, origin.execution_id);
        debug_assert_eq!(
            attribution.agent.map(|agent| agent.thread_id),
            origin.thread_id
        );
        debug_assert_eq!(attribution.agent.map(|agent| agent.turn_id), origin.turn_id);
        Self {
            scope,
            attribution,
            origin,
            recorder,
        }
    }

    pub fn scope(&self) -> &ExecutionScope {
        &self.scope
    }

    pub(crate) fn recorder(&self) -> Arc<dyn ToolExecutionRecorder> {
        self.recorder.clone()
    }

    pub fn attribution(&self) -> ExecutionAttribution {
        self.attribution
    }
}

/// Tool registry and safety path bound immutably to one execution.
///
/// The public execution API needs only a workspace and a canonical call.
/// Agent presentation and control-plane enrichment are supplied by the thin
/// adapter in `tool_orchestrator` and are not prerequisites for this handle.
#[derive(Clone)]
pub struct BoundTools {
    registry: ToolRegistry,
    policy: Arc<dyn ApprovalPolicy>,
    approval: Arc<dyn ApprovalTransport>,
    permission_grants: Arc<ExecutionPermissionGrants>,
    binding: ToolExecutionBinding,
    default_timeout_ms: u64,
    max_output_bytes: usize,
    hooks: Arc<dyn crate::contracts::ExecutionHooks>,
    operations: Option<Arc<dyn crate::contracts::ToolHost>>,
    conversation: Option<Arc<dyn crate::contracts::ConversationReader>>,
    conversation_session_id: Option<crate::domain::SessionId>,
}

impl BoundTools {
    pub fn new(
        registry: ToolRegistry,
        policy: Arc<dyn ApprovalPolicy>,
        approval: Arc<dyn ApprovalTransport>,
        permission_grants: Arc<ExecutionPermissionGrants>,
        binding: ToolExecutionBinding,
    ) -> Self {
        Self {
            registry,
            policy,
            approval,
            permission_grants,
            binding,
            default_timeout_ms: DEFAULT_TIMEOUT_MS,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            hooks: Arc::new(crate::contracts::NoExecutionHooks),
            operations: None,
            conversation: None,
            conversation_session_id: None,
        }
    }

    pub(crate) fn with_limits(mut self, default_timeout_ms: u64, max_output_bytes: usize) -> Self {
        self.default_timeout_ms = default_timeout_ms;
        self.max_output_bytes = max_output_bytes;
        self
    }

    pub fn binding(&self) -> &ToolExecutionBinding {
        &self.binding
    }

    pub fn with_hooks(mut self, hooks: Arc<dyn crate::contracts::ExecutionHooks>) -> Self {
        self.hooks = hooks;
        self
    }

    pub fn with_operations(
        mut self,
        operations: Option<Arc<dyn crate::contracts::ToolHost>>,
    ) -> Self {
        self.operations = operations;
        self
    }

    pub fn with_conversation(
        mut self,
        conversation: Option<Arc<dyn crate::contracts::ConversationReader>>,
        session_id: Option<crate::domain::SessionId>,
    ) -> Self {
        self.conversation = conversation;
        self.conversation_session_id = session_id;
        self
    }

    pub fn visible_specs(&self, cwd: &Path) -> Vec<ToolSpec> {
        self.registry
            .specs()
            .into_iter()
            .filter(|spec| {
                if !self
                    .registry
                    .get(&spec.name)
                    .is_some_and(|tool| tool.model_visible())
                {
                    return false;
                }
                visibility_decision_allows(
                    spec,
                    self.policy
                        .evaluate_visibility(&PolicyVisibilityContext::new(
                            cwd.to_path_buf(),
                            spec.clone(),
                        )),
                    self.approval.can_request_approval(),
                )
            })
            .collect()
    }

    pub(crate) fn validate_model_tools(&self, tools: &[ToolSpec], cwd: &Path) -> Result<()> {
        let mut names = std::collections::HashSet::new();
        for tool in tools {
            anyhow::ensure!(
                names.insert(&tool.name),
                "duplicate model tool: {}",
                tool.name
            );
            let registered = if matches!(
                tool.surface,
                crate::domain::ToolSurface::WorkflowFunction { .. }
            ) {
                anyhow::ensure!(
                    self.registry.get(&tool.name).is_none(),
                    "workflow tool '{}' shadows a registered host tool",
                    tool.name
                );
                crate::domain::tool_validation::validate_tool_input_schema(tool)
                    .map_err(anyhow::Error::msg)?;
                tool.clone()
            } else {
                let registered = self.registry.spec(&tool.name)?;
                anyhow::ensure!(
                    self.registry
                        .get(&tool.name)
                        .is_some_and(|tool| tool.model_visible()),
                    "model request contains user-only tool '{}'",
                    tool.name
                );
                anyhow::ensure!(
                    registered == *tool,
                    "model request changed registered tool '{}'",
                    tool.name
                );
                registered
            };
            anyhow::ensure!(
                visibility_decision_allows(
                    tool,
                    self.policy
                        .evaluate_visibility(&PolicyVisibilityContext::new(
                            cwd.to_path_buf(),
                            registered
                        )),
                    self.approval.can_request_approval(),
                ),
                "model request contains policy-hidden tool '{}'",
                tool.name
            );
        }
        Ok(())
    }

    pub async fn execute(&self, cwd: PathBuf, call: ToolCall) -> Result<ToolResult> {
        self.execute_enriched(cwd, call, &NoopToolExecutionObserver, |_| {})
            .await
    }

    pub(crate) async fn execute_enriched<F>(
        &self,
        cwd: PathBuf,
        mut call: ToolCall,
        observer: &dyn ToolExecutionObserver,
        enrich: F,
    ) -> Result<ToolResult>
    where
        F: FnOnce(&mut ToolContext),
    {
        use super::model_call_scope::*;
        let cancellation = operation_cancellation(&self.binding.scope.cancellation);
        anyhow::ensure!(!cancellation.is_cancelled(), "tool execution canceled");
        let mut lineage = current_tool_lineage();
        if let Some(tool) = self.registry.get(&call.name) {
            let identity = Arc::as_ptr(&tool) as *const () as usize;
            anyhow::ensure!(
                !lineage.contains(&identity),
                "nested tool cycle for registry tool '{}'",
                call.name
            );
            lineage.push(identity);
        }
        if let Some(raw_arguments) = call.raw_arguments.as_deref()
            && let Ok(parsed_arguments) = serde_json::from_str(raw_arguments)
        {
            call.args = parsed_arguments;
        }
        self.binding
            .recorder
            .tool_call_requested(self.binding.attribution, current_operation_owner(), &call)
            .await?;
        with_operation_scope(
            crate::contracts::OperationOwner::Tool {
                call_id: call.id.clone(),
            },
            lineage,
            cancellation,
            self.execute_requested(cwd, call, observer, enrich),
        )
        .await
    }

    async fn execute_requested<F>(
        &self,
        cwd: PathBuf,
        mut call: ToolCall,
        observer: &dyn ToolExecutionObserver,
        enrich: F,
    ) -> Result<ToolResult>
    where
        F: FnOnce(&mut ToolContext),
    {
        if super::model_call_scope::operation_cancellation(&self.binding.scope.cancellation)
            .is_cancelled()
        {
            anyhow::bail!("tool execution canceled");
        }

        observer.tool_call_requested(&call).await?;

        let tool_spec = self.registry.spec(&call.name).ok();
        if let Some(spec) = tool_spec.as_ref()
            && let Some(error) = validate_tool_call_args(&call, spec)
        {
            self.record_resolution(
                &call,
                &ToolCallResolution::ValidationFailed {
                    reason: error.clone(),
                },
            )
            .await?;
            let result = ToolResult::error(call.id.clone(), error).with_metadata(json!({
                "tool": call.name,
                "validation_error": true,
            }));
            return self.finish(observer, &cwd, &call, result).await;
        }

        if let Some(result) = self
            .enforce_before_tool(observer, &cwd, &mut call, tool_spec.clone())
            .await?
        {
            return Ok(result);
        }

        if super::model_call_scope::operation_cancellation(&self.binding.scope.cancellation)
            .is_cancelled()
        {
            anyhow::bail!("tool execution canceled");
        }
        let decision = self.evaluate_access(&cwd, &call, tool_spec.clone());
        let mut enrich = Some(enrich);
        match decision {
            PolicyDecision::Allow => {
                self.record_resolution(&call, &ToolCallResolution::Allowed)
                    .await?;
                self.invoke_allowed(
                    observer,
                    &call,
                    tool_spec,
                    cwd,
                    enrich.take().expect("tool context enrichment"),
                )
                .await
            }
            PolicyDecision::Ask { reason } => {
                self.binding
                    .recorder
                    .tool_approval_requested(self.binding.attribution, &call, &reason)
                    .await?;
                observer.approval_requested(&call, &reason).await?;
                let approval_request = self.approval.request_approval(
                    ApprovalRequest::new(
                        call.clone(),
                        cwd.clone(),
                        reason.clone(),
                        tool_spec.clone(),
                    )
                    .with_origin(self.binding.origin.clone()),
                );
                let parent_cancellation = super::model_call_scope::operation_cancellation(
                    &self.binding.scope.cancellation,
                );
                let approval = tokio::select! {
                    result = approval_request => result?,
                    _ = parent_cancellation.cancelled() => {
                        return Err(anyhow!("tool execution canceled"));
                    }
                };
                observer.approval_resolved(&call, approval.approved).await?;
                if approval.approved {
                    self.record_resolution(&call, &ToolCallResolution::Approved)
                        .await?;
                    let result = self
                        .invoke_allowed(
                            observer,
                            &call,
                            tool_spec,
                            cwd,
                            enrich.take().expect("tool context enrichment"),
                        )
                        .await?;
                    self.merge_granted_permissions(&result);
                    return Ok(result);
                }

                let result = ToolResult::error(
                    call.id.clone(),
                    approval
                        .note
                        .unwrap_or_else(|| format!("tool call was not approved: {reason}")),
                );
                self.record_resolution(
                    &call,
                    &ToolCallResolution::ApprovalDenied {
                        reason: result.text_or_status(),
                    },
                )
                .await?;
                self.finish(observer, &cwd, &call, result).await
            }
            PolicyDecision::Deny { reason } => {
                self.record_resolution(
                    &call,
                    &ToolCallResolution::PolicyDenied {
                        reason: reason.clone(),
                    },
                )
                .await?;
                self.finish(
                    observer,
                    &cwd,
                    &call,
                    ToolResult::error(call.id.clone(), reason),
                )
                .await
            }
            other => {
                let reason = format!("unsupported policy decision: {other:?}");
                self.record_resolution(
                    &call,
                    &ToolCallResolution::Unsupported {
                        reason: reason.clone(),
                    },
                )
                .await?;
                self.finish(
                    observer,
                    &cwd,
                    &call,
                    ToolResult::error(call.id.clone(), reason),
                )
                .await
            }
        }
    }

    fn evaluate_access(
        &self,
        cwd: &Path,
        call: &ToolCall,
        tool_spec: Option<ToolSpec>,
    ) -> PolicyDecision {
        let Some(spec) = tool_spec else {
            return PolicyDecision::Deny {
                reason: format!("unknown tool: {}", call.name),
            };
        };
        self.policy.evaluate(
            call,
            &PolicyContext::new(cwd.to_path_buf(), Some(spec))
                .with_granted_permissions(self.permission_grants.snapshot()),
        )
    }

    async fn record_resolution(
        &self,
        call: &ToolCall,
        resolution: &ToolCallResolution,
    ) -> Result<()> {
        self.binding
            .recorder
            .tool_call_resolved(self.binding.attribution, call, resolution)
            .await
    }

    async fn record_result(
        &self,
        observer: &dyn ToolExecutionObserver,
        result: ToolResult,
    ) -> Result<ToolResult> {
        self.binding
            .recorder
            .tool_result_recorded(self.binding.attribution, &result)
            .await?;
        observer.tool_finished(&result).await?;
        Ok(result)
    }

    fn truncate_result(&self, mut result: ToolResult) -> ToolResult {
        let (output, output_truncated, output_original_bytes) =
            truncate_utf8(result.output, self.max_output_bytes, "output");
        result.output = output;

        let (error, error_truncated, error_original_bytes) = result
            .error
            .map(|error| truncate_utf8(error, self.max_output_bytes, "error"))
            .map(|(error, truncated, original_bytes)| (Some(error), truncated, original_bytes))
            .unwrap_or((None, false, 0));
        result.error = error;

        if output_truncated || error_truncated {
            let mut metadata = result.metadata;
            if output_truncated {
                metadata = metadata_with(metadata, "output_truncated", json!(true));
                metadata = metadata_with(
                    metadata,
                    "output_original_bytes",
                    json!(output_original_bytes),
                );
            }
            if error_truncated {
                metadata = metadata_with(metadata, "error_truncated", json!(true));
                metadata = metadata_with(
                    metadata,
                    "error_original_bytes",
                    json!(error_original_bytes),
                );
            }
            metadata = metadata_with(metadata, "max_output_bytes", json!(self.max_output_bytes));
            result.metadata = metadata;
        }

        result
    }

    fn merge_granted_permissions(&self, result: &ToolResult) {
        if !result.ok {
            return;
        }
        let Some(permissions) = result.metadata.get("granted_permissions") else {
            return;
        };
        let Some(permissions) = permissions.as_array() else {
            return;
        };
        self.permission_grants.grant(
            permissions
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
    }
}

#[async_trait]
pub(crate) trait ToolExecutionObserver: Send + Sync {
    async fn tool_call_requested(&self, call: &ToolCall) -> Result<()>;
    async fn tool_call_resolved(&self, _call: &ToolCall) -> Result<()> {
        Ok(())
    }
    async fn approval_requested(&self, call: &ToolCall, reason: &str) -> Result<()>;
    async fn approval_resolved(&self, call: &ToolCall, approved: bool) -> Result<()>;
    async fn tool_finished(&self, result: &ToolResult) -> Result<()>;
}

pub(crate) struct NoopToolExecutionObserver;

#[async_trait]
impl ToolExecutionObserver for NoopToolExecutionObserver {
    async fn tool_call_requested(&self, _call: &ToolCall) -> Result<()> {
        Ok(())
    }

    async fn approval_requested(&self, _call: &ToolCall, _reason: &str) -> Result<()> {
        Ok(())
    }

    async fn approval_resolved(&self, _call: &ToolCall, _approved: bool) -> Result<()> {
        Ok(())
    }

    async fn tool_finished(&self, _result: &ToolResult) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "bound_tools_tests.rs"]
mod tests;
