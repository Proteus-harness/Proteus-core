//! Optional interactive terminal capability of an existing tool export.
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

pub const TOOL_TERMINAL_FEATURE: &str = "tool_terminal/v1";
pub const PROCESS_TOOL_TERMINAL_METHOD: &str = "terminal_control";
pub const TOOL_HOST_PUBLISH_TERMINAL_METHOD: &str = "tool.publish_terminal";
pub const TOOL_TERMINAL_MAX_BYTES: u32 = 65_536;
pub const TOOL_TERMINAL_MAX_WAIT_MS: u32 = 1_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ToolTerminalDescriptor {
    pub handle: String,
    pub title: String,
    pub tty: bool,
    pub columns: u16,
    pub rows: u16,
}

// Intentionally no Debug: input may contain a password.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ToolTerminalRequest {
    pub handle: String,
    pub action: ToolTerminalAction,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolTerminalAction {
    Read {
        cursor: Option<u64>,
        max_bytes: u32,
        wait_ms: u32,
    },
    Input {
        data_base64: String,
    },
    Resize {
        columns: u16,
        rows: u16,
    },
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ToolTerminalSnapshot {
    pub start_cursor: u64,
    pub next_cursor: u64,
    pub end_cursor: u64,
    pub data_base64: String,
    pub gap: bool,
    /// True only after process exit and complete output-reader drain.
    pub finished: bool,
    pub exit_code: Option<i32>,
    pub tty: bool,
    pub columns: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "snapshot", rename_all = "snake_case")]
pub enum ToolTerminalResponse {
    Read(ToolTerminalSnapshot),
    Accepted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ToolTerminalRef {
    pub id: Uuid,
    pub title: String,
    pub tty: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessToolTerminalInput {
    pub request: ToolTerminalRequest,
    pub context: crate::process_module::ToolModuleInvocationContext,
}

#[async_trait]
pub trait ToolTerminalController: Send + Sync {
    async fn control(&self, action: ToolTerminalAction) -> Result<ToolTerminalResponse>;
}

/// Runtime-owned registration; clients never receive the private module handle.
#[async_trait]
pub trait ToolTerminalHost: Send + Sync {
    async fn publish(
        &self,
        descriptor: ToolTerminalDescriptor,
        controller: Arc<dyn ToolTerminalController>,
    ) -> Result<ToolTerminalRef>;
}
