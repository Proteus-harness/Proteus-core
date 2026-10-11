use std::path::PathBuf;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::domain::{AgentTask, ToolSpec};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ToolSelectionRequest {
    pub task: AgentTask,
    pub cwd: PathBuf,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub max_tools: Option<usize>,
    #[serde(default)]
    pub reason: Option<String>,
    /// Фаза workflow ("plan"/"execute"/"review"/...), если workflow фазовый.
    /// Selector может использовать её для phase-aware отбора tools.
    #[serde(default)]
    pub phase: Option<String>,
}

impl ToolSelectionRequest {
    pub fn new(task: AgentTask) -> Self {
        Self {
            cwd: task.cwd.clone(),
            task,
            query: None,
            max_tools: None,
            reason: None,
            phase: None,
        }
    }

    pub fn with_query(mut self, query: impl Into<String>) -> Self {
        self.query = Some(query.into());
        self
    }

    pub fn with_max_tools(mut self, max_tools: usize) -> Self {
        self.max_tools = Some(max_tools);
        self
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    pub fn with_phase(mut self, phase: impl Into<String>) -> Self {
        self.phase = Some(phase.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ToolSelectionInput {
    pub request: ToolSelectionRequest,
    pub candidates: Vec<ToolSpec>,
}

impl ToolSelectionInput {
    pub fn new(request: ToolSelectionRequest, candidates: Vec<ToolSpec>) -> Self {
        Self {
            request,
            candidates,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ToolSelectionOutput {
    pub tools: Vec<ToolSpec>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl ToolSelectionOutput {
    pub fn new(tools: Vec<ToolSpec>) -> Self {
        Self {
            tools,
            metadata: serde_json::Value::Null,
        }
    }

    /// Selection can reorder or omit candidates, but cannot redefine tools.
    pub fn validate_against(&self, candidates: &[ToolSpec]) -> Result<()> {
        let mut names = std::collections::HashSet::new();
        for tool in &self.tools {
            ensure!(
                names.insert(&tool.name),
                "duplicate selected tool: {}",
                tool.name
            );
            ensure!(
                candidates.iter().any(|candidate| candidate == tool),
                "tool selection changed or invented registered tool '{}'",
                tool.name
            );
        }
        Ok(())
    }
}
