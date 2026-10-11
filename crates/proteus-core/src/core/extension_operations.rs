//! Invocation-bound extension callbacks. The facade retains no strong hook cycle.
use super::{
    BoundTools,
    model_call_scope::{with_model_call_origin, with_operation_scope},
};
use crate::{
    contracts::{
        CancellationToken, ExecutionHooks, HookEvent, HookHost, HookInput, Model, ModelCallOrigin,
        OperationOwner, ToolHost,
    },
    domain::{ToolCall, ToolResult},
    model_standard::{CanonicalModelRequest, CanonicalModelResponse},
};
use anyhow::{Result, anyhow, ensure};
use async_trait::async_trait;
use std::{
    path::PathBuf,
    sync::{Arc, Weak},
};

#[derive(Clone)]
pub(crate) struct ExtensionOperations {
    pub model: Option<Arc<dyn Model>>,
    pub tools: BoundTools,
}

pub(crate) struct InvocationOperations {
    pub operations: Option<ExtensionOperations>,
    pub owner: OperationOwner,
    pub lineage: Vec<usize>,
    pub cancellation: CancellationToken,
    pub cwd: PathBuf,
    pub model_allowed: bool,
    pub tools_allowed: bool,
}
impl InvocationOperations {
    async fn model(&self, request: CanonicalModelRequest) -> Result<CanonicalModelResponse> {
        ensure!(
            self.model_allowed,
            "model callback is unavailable in lifecycle notifications"
        );
        let model = self
            .operations
            .as_ref()
            .and_then(|ops| ops.model.as_ref())
            .ok_or_else(|| anyhow!("no model is configured for this execution"))?;
        // Captured scope is restored here because broker callbacks run in spawned tasks.
        with_operation_scope(
            self.owner.clone(),
            self.lineage.clone(),
            self.cancellation.clone(),
            with_model_call_origin(ModelCallOrigin::Auxiliary, model.complete(request)),
        )
        .await
    }
    async fn tool(&self, call: ToolCall) -> Result<ToolResult> {
        ensure!(
            self.tools_allowed,
            "nested tool callback is unavailable in this hook phase"
        );
        let operations = self
            .operations
            .as_ref()
            .ok_or_else(|| anyhow!("tool callbacks are not bound"))?;
        with_operation_scope(
            self.owner.clone(),
            self.lineage.clone(),
            self.cancellation.clone(),
            operations.tools.execute(self.cwd.clone(), call),
        )
        .await
    }
}
#[async_trait]
impl HookHost for InvocationOperations {
    async fn complete_model(
        &self,
        request: CanonicalModelRequest,
    ) -> Result<CanonicalModelResponse> {
        self.model(request).await
    }
    async fn execute_tool(&self, call: ToolCall) -> Result<ToolResult> {
        self.tool(call).await
    }
    fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}
#[async_trait]
impl ToolHost for InvocationOperations {
    async fn complete_model(
        &self,
        request: CanonicalModelRequest,
    ) -> Result<CanonicalModelResponse> {
        let model = self
            .operations
            .as_ref()
            .and_then(|ops| ops.model.as_ref())
            .ok_or_else(|| anyhow!("no model is configured"))?;
        with_model_call_origin(ModelCallOrigin::Auxiliary, model.complete(request)).await
    }
    async fn execute_tool(&self, call: ToolCall) -> Result<ToolResult> {
        self.operations
            .as_ref()
            .ok_or_else(|| anyhow!("tool callbacks are not bound"))?
            .tools
            .execute(self.cwd.clone(), call)
            .await
    }
    fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

pub(crate) struct WeakExecutionHooks(pub Weak<dyn ExecutionHooks>);
#[async_trait]
impl ExecutionHooks for WeakExecutionHooks {
    fn is_active(&self) -> bool {
        self.0.upgrade().is_some_and(|hooks| hooks.is_active())
    }
    async fn apply(&self, input: HookInput) -> Result<HookEvent> {
        let hooks = self
            .0
            .upgrade()
            .ok_or_else(|| anyhow!("execution hook binding has expired"))?;
        hooks.apply(input).await
    }
}

/// The active tool supplies lineage and cancellation rather than trusting a module.
pub(crate) struct DelegatingToolHost {
    pub delegate: Option<Arc<dyn ToolHost>>,
    pub tools: BoundTools,
    pub cwd: PathBuf,
    pub owner: OperationOwner,
    pub lineage: Vec<usize>,
    pub cancellation: CancellationToken,
}
#[async_trait]
impl ToolHost for DelegatingToolHost {
    async fn complete_model(
        &self,
        request: CanonicalModelRequest,
    ) -> Result<CanonicalModelResponse> {
        let host = self
            .delegate
            .as_ref()
            .ok_or_else(|| anyhow!("model callbacks are not bound"))?;
        with_operation_scope(
            self.owner.clone(),
            self.lineage.clone(),
            self.cancellation.clone(),
            host.complete_model(request),
        )
        .await
    }
    async fn execute_tool(&self, call: ToolCall) -> Result<ToolResult> {
        with_operation_scope(
            self.owner.clone(),
            self.lineage.clone(),
            self.cancellation.clone(),
            self.tools.execute(self.cwd.clone(), call),
        )
        .await
    }
    fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

pub(crate) struct WeakToolHost(pub Weak<InvocationOperations>);
#[async_trait]
impl ToolHost for WeakToolHost {
    async fn complete_model(
        &self,
        request: CanonicalModelRequest,
    ) -> Result<CanonicalModelResponse> {
        let host = self
            .0
            .upgrade()
            .ok_or_else(|| anyhow!("execution operation binding has expired"))?;
        ToolHost::complete_model(host.as_ref(), request).await
    }
    async fn execute_tool(&self, call: ToolCall) -> Result<ToolResult> {
        let host = self
            .0
            .upgrade()
            .ok_or_else(|| anyhow!("execution operation binding has expired"))?;
        ToolHost::execute_tool(host.as_ref(), call).await
    }
}

#[cfg(test)]
#[path = "extension_operations_tests.rs"]
mod tests;
