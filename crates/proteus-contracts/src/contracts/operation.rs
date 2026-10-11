//! Invocation-bound nested operations; modules cannot select their owner.
use crate::{
    domain::{CallId, ToolCall, ToolResult},
    model_standard::{CanonicalModelRequest, CanonicalModelResponse},
};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub const OPERATION_HOST_COMPLETE_MODEL_METHOD: &str = "host.model.complete";
pub const OPERATION_HOST_EXECUTE_TOOL_METHOD: &str = "host.tools.execute";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "owner", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperationOwner {
    Workflow,
    Hook { invocation_id: uuid::Uuid },
    Tool { call_id: CallId },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OperationCompleteModelRequest {
    pub request: CanonicalModelRequest,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OperationExecuteToolRequest {
    pub call: ToolCall,
}

#[async_trait]
pub trait HookHost: Send + Sync {
    fn is_cancelled(&self) -> bool {
        false
    }
    async fn complete_model(
        &self,
        request: CanonicalModelRequest,
    ) -> Result<CanonicalModelResponse>;
    async fn execute_tool(&self, call: ToolCall) -> Result<ToolResult>;
}
#[async_trait]
pub trait ToolHost: Send + Sync {
    fn is_cancelled(&self) -> bool {
        false
    }
    async fn complete_model(
        &self,
        request: CanonicalModelRequest,
    ) -> Result<CanonicalModelResponse>;
    async fn execute_tool(&self, call: ToolCall) -> Result<ToolResult>;
}
