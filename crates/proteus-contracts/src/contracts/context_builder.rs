use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    contracts::{ExecutionAttribution, ExecutionScope},
    domain::{AgentTask, ContextBundle},
};

#[derive(Clone)]
pub struct ContextBuildInput {
    pub task: AgentTask,
    pub scope: ExecutionScope,
    pub attribution: ExecutionAttribution,
}

impl ContextBuildInput {
    pub fn new(task: AgentTask, scope: ExecutionScope, attribution: ExecutionAttribution) -> Self {
        Self {
            task,
            scope,
            attribution,
        }
    }
}

/// Canonical result of one execution-bound context build.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContextBuildOutcome {
    Success {
        bundle: ContextBundle,
    },
    Error {
        message: String,
        /// Error sources in their original order, excluding the top message.
        causes: Vec<String>,
    },
}

#[async_trait]
pub trait ContextBuilder: Send + Sync {
    async fn build(&self, input: ContextBuildInput) -> Result<ContextBundle>;
}
