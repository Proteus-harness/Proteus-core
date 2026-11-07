use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    domain::{
        CacheHints, ModelLimits, ModelRef, ReasoningConfig, ResponseFormat, SamplingConfig,
        ToolChoice, ToolSpec,
    },
    model_standard::CanonicalMessage,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct CanonicalModelRequest {
    pub model: ModelRef,
    pub instructions: Vec<InstructionBlock>,
    pub messages: Vec<CanonicalMessage>,
    pub tools: Vec<ToolSpec>,
    pub tool_choice: ToolChoice,
    pub response_format: ResponseFormat,
    pub sampling: SamplingConfig,
    pub reasoning: ReasoningConfig,
    pub limits: ModelLimits,
    pub cache: CacheHints,
    #[serde(default)]
    pub client_metadata: BTreeMap<String, String>,
    pub metadata: serde_json::Value,
}

impl CanonicalModelRequest {
    /// Минимальный конструктор с defaults. Остальные поля через `with_*`.
    pub fn new(model: ModelRef, messages: Vec<CanonicalMessage>) -> Self {
        Self {
            model,
            instructions: Vec::new(),
            messages,
            tools: Vec::new(),
            tool_choice: ToolChoice::default(),
            response_format: ResponseFormat::default(),
            sampling: SamplingConfig::default(),
            reasoning: ReasoningConfig::default(),
            limits: ModelLimits::default(),
            cache: CacheHints::default(),
            client_metadata: BTreeMap::new(),
            metadata: serde_json::Value::Null,
        }
    }

    pub fn with_instructions(mut self, instructions: Vec<InstructionBlock>) -> Self {
        self.instructions = instructions;
        self
    }
    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }
    pub fn with_tool_choice(mut self, tool_choice: ToolChoice) -> Self {
        self.tool_choice = tool_choice;
        self
    }
    pub fn with_response_format(mut self, response_format: ResponseFormat) -> Self {
        self.response_format = response_format;
        self
    }
    pub fn with_sampling(mut self, sampling: SamplingConfig) -> Self {
        self.sampling = sampling;
        self
    }
    pub fn with_reasoning(mut self, reasoning: ReasoningConfig) -> Self {
        self.reasoning = reasoning;
        self
    }
    pub fn with_limits(mut self, limits: ModelLimits) -> Self {
        self.limits = limits;
        self
    }
    pub fn with_cache(mut self, cache: CacheHints) -> Self {
        self.cache = cache;
        self
    }
    pub fn with_client_metadata(mut self, client_metadata: BTreeMap<String, String>) -> Self {
        self.client_metadata = client_metadata;
        self
    }
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct InstructionBlock {
    pub kind: InstructionKind,
    pub text: String,
    pub priority: u8,
}

impl InstructionBlock {
    pub fn new(kind: InstructionKind, text: impl Into<String>, priority: u8) -> Self {
        Self {
            kind,
            text: text.into(),
            priority,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub enum InstructionKind {
    System,
    Developer,
    Project,
    UserPreference,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::WorkflowCompleteModelRequest;
    use crate::model_standard::MessageRole;

    #[test]
    fn strict_workflow_model_request_rejects_unknown_fields_at_each_known_layer() {
        let request = CanonicalModelRequest::new(
            ModelRef::new("fake", "test"),
            vec![CanonicalMessage::text(MessageRole::User, "test")],
        )
        .with_instructions(vec![InstructionBlock::new(
            InstructionKind::System,
            "test",
            1,
        )])
        .with_metadata(serde_json::json!({"custom":{"extra":true}}));
        let valid = serde_json::to_value(WorkflowCompleteModelRequest { request }).unwrap();
        assert!(serde_json::from_value::<WorkflowCompleteModelRequest>(valid.clone()).is_ok());
        for layer in 0..5 {
            let mut invalid = valid.clone();
            match layer {
                0 => invalid["request"]["reasoning"] = serde_json::json!({"budget_token":1024}),
                1 => invalid["request"]["extra"] = serde_json::json!(true),
                2 => invalid["request"]["instructions"][0]["extra"] = serde_json::json!(true),
                3 => invalid["request"]["messages"][0]["extra"] = serde_json::json!(true),
                4 => {
                    invalid["request"]["messages"][0]["parts"][0]["payload"]["Text"]["extra"] =
                        serde_json::json!(true)
                }
                _ => unreachable!(),
            }
            assert!(
                serde_json::from_value::<WorkflowCompleteModelRequest>(invalid).is_err(),
                "layer {layer}"
            );
        }
    }
}
