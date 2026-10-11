//! Ordered, execution-bound contributions at typed runtime boundaries.
use super::{ExecutionAttribution, ModelCallOrigin};
use crate::{
    domain::{AgentOutput, AgentTask, ToolCall, ToolResult, ToolSpec, validate_tool_call_args},
    model_standard::{CanonicalMessage, CanonicalModelRequest, InstructionBlock},
};
use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

pub const PROCESS_HOOK_CONTRACT_VERSION: &str = "v6";
pub const PROCESS_HOOK_INVOKE_METHOD: &str = "hook.invoke";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HookInput {
    pub event: HookEvent,
    pub attribution: ExecutionAttribution,
    pub cwd: PathBuf,
    /// Read-only conversation facts. Never changed by a hook response.
    #[serde(deserialize_with = "Option::deserialize")]
    pub conversation: Option<super::ConversationSnapshot>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum HookEvent {
    TurnStarted {
        task: AgentTask,
        history: Vec<CanonicalMessage>,
    },
    BeforeToolSelection {
        input: super::ToolSelectionInput,
        output: super::ToolSelectionOutput,
    },
    BeforeContext {
        input: super::ContextPreparationInput,
        output: super::ContextPreparationOutput,
        #[serde(deserialize_with = "Option::deserialize")]
        target: Option<super::ContextPreparationTarget>,
    },
    BeforeModel {
        origin: ModelCallOrigin,
        request: CanonicalModelRequest,
    },
    BeforeTool {
        call: ToolCall,
        spec: Option<ToolSpec>,
        blocked: Option<String>,
    },
    AfterTool {
        call: ToolCall,
        result: ToolResult,
    },
    BeforeStop {
        task: AgentTask,
        history: Vec<CanonicalMessage>,
        output: AgentOutput,
        attempt: u32,
        continuation: Option<String>,
    },
    TurnSettled {
        status: HookTurnStatus,
        output: Option<AgentOutput>,
        error: Option<String>,
    },
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookTurnStatus {
    Success,
    Error,
    Canceled,
    Timeout,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum HookResponse {
    Continue,
    ToolSelection {
        output: super::ToolSelectionOutput,
    },
    ContextPreparation {
        output: super::ContextPreparationOutput,
    },
    ModelContext {
        messages: Vec<CanonicalMessage>,
        instructions: Vec<InstructionBlock>,
    },
    ToolArguments {
        args: serde_json::Value,
    },
    ContinueTurn {
        reason: String,
    },
    BlockTool {
        reason: String,
    },
    ToolOutput {
        output: String,
    },
}
impl<'de> Deserialize<'de> for HookResponse {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            Continue {},
            ToolSelection {
                output: super::ToolSelectionOutput,
            },
            ContextPreparation {
                output: super::ContextPreparationOutput,
            },
            ModelContext {
                messages: Vec<CanonicalMessage>,
                instructions: Vec<InstructionBlock>,
            },
            ToolArguments {
                args: serde_json::Value,
            },
            ContinueTurn {
                reason: String,
            },
            BlockTool {
                reason: String,
            },
            ToolOutput {
                output: String,
            },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Continue {} => Self::Continue,
            Wire::ToolSelection { output } => Self::ToolSelection { output },
            Wire::ContextPreparation { output } => Self::ContextPreparation { output },
            Wire::ModelContext {
                messages,
                instructions,
            } => Self::ModelContext {
                messages,
                instructions,
            },
            Wire::ToolArguments { args } => Self::ToolArguments { args },
            Wire::ContinueTurn { reason } => Self::ContinueTurn { reason },
            Wire::BlockTool { reason } => Self::BlockTool { reason },
            Wire::ToolOutput { output } => Self::ToolOutput { output },
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProcessHookResponse {
    pub result: HookResponse,
}

pub fn apply_hook_response(event: &HookEvent, response: &HookResponse) -> Result<HookEvent> {
    let mut event = event.clone();
    match (&mut event, response) {
        (_, HookResponse::Continue) => {}
        (
            HookEvent::BeforeToolSelection { input, output },
            HookResponse::ToolSelection { output: candidate },
        ) => {
            candidate.validate_against(&input.candidates)?;
            candidate.validate_against(&output.tools)?;
            *output = candidate.clone();
        }
        (
            HookEvent::BeforeContext { input, output, .. },
            HookResponse::ContextPreparation { output: candidate },
        ) => {
            *output = super::context_preparation::compose_outputs(input, output, candidate)?;
        }
        (
            HookEvent::BeforeModel { request, .. },
            HookResponse::ModelContext {
                messages,
                instructions,
            },
        ) => {
            validate_model_context(&request.messages, messages)?;
            request.messages = messages.clone();
            request.instructions = instructions.clone();
        }
        (HookEvent::BeforeTool { call, spec, .. }, HookResponse::ToolArguments { args }) => {
            call.args = args.clone();
            call.raw_arguments = None;
            if let Some(spec) = spec
                && let Some(error) = validate_tool_call_args(call, spec)
            {
                bail!("invalid hook tool arguments: {error}");
            }
        }
        (HookEvent::BeforeStop { continuation, .. }, HookResponse::ContinueTurn { reason })
            if !reason.trim().is_empty() =>
        {
            *continuation = Some(reason.clone());
        }
        (HookEvent::BeforeTool { blocked, .. }, HookResponse::BlockTool { reason })
            if !reason.trim().is_empty() =>
        {
            *blocked = Some(reason.clone());
        }
        (HookEvent::AfterTool { result, .. }, HookResponse::ToolOutput { output }) => {
            result.output = output.clone()
        }
        _ => bail!("hook response is not allowed for this event, or block reason is blank"),
    }
    Ok(event)
}
fn validate_model_context(
    original: &[CanonicalMessage],
    messages: &[CanonicalMessage],
) -> Result<()> {
    use std::collections::{HashMap, HashSet};
    let original_parts = original
        .iter()
        .flat_map(|m| &m.parts)
        .map(|p| (p.part_id, p))
        .collect::<HashMap<_, _>>();
    let original_messages = original
        .iter()
        .map(|m| (m.id, m))
        .collect::<HashMap<_, _>>();
    let mut message_ids = HashSet::new();
    let mut parts = HashMap::new();
    for message in messages {
        if !message_ids.insert(message.id) {
            bail!("hook model context has duplicate message id {}", message.id);
        }
        if let Some(previous) = original_messages.get(&message.id) {
            if message.role != previous.role
                || message.phase != previous.phase
                || message.name != previous.name
                || message.tool_call_id != previous.tool_call_id
                || message.metadata != previous.metadata
            {
                bail!(
                    "hook model context changes existing message identity {}",
                    message.id
                );
            }
        }
        for part in &message.parts {
            if let Some(previous) = original_parts.get(&part.part_id) {
                if *previous != part {
                    bail!("hook model context changes immutable part {}", part.part_id);
                }
            }
            if let Some(previous) = parts.insert(part.part_id, part) {
                if previous != part {
                    bail!(
                        "hook model context has conflicting part id {}",
                        part.part_id
                    );
                }
            }
        }
    }
    Ok(())
}

#[async_trait]
pub trait HookHandler: Send + Sync {
    async fn invoke(
        &self,
        input: HookInput,
        host: Arc<dyn super::HookHost>,
    ) -> Result<HookResponse>;
}
#[async_trait]
pub trait ExecutionHooks: Send + Sync {
    fn is_active(&self) -> bool {
        true
    }
    async fn apply(&self, input: HookInput) -> Result<HookEvent>;
}
#[derive(Debug, Default)]
pub struct NoExecutionHooks;
#[async_trait]
impl ExecutionHooks for NoExecutionHooks {
    fn is_active(&self) -> bool {
        false
    }
    async fn apply(&self, input: HookInput) -> Result<HookEvent> {
        Ok(input.event)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HookTrace {
    pub owner: super::OperationOwner,
    pub input: HookInput,
    pub steps: Vec<HookStep>,
    pub output: Option<HookEvent>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HookStep {
    pub invocation_id: uuid::Uuid,
    pub module_id: String,
    pub outcome: HookStepOutcome,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum HookStepOutcome {
    Accepted {
        response: HookResponse,
    },
    Failed {
        message: String,
        #[serde(deserialize_with = "Option::deserialize")]
        model_failure: Option<crate::model_standard::ModelFailure>,
    },
}

#[cfg(test)]
mod tests;
