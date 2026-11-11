use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

impl ModelRef {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct SamplingConfig {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
}

impl SamplingConfig {
    pub fn new(temperature: Option<f32>, top_p: Option<f32>) -> Self {
        Self { temperature, top_p }
    }
}

impl Default for SamplingConfig {
    fn default() -> Self {
        Self {
            temperature: Some(0.0),
            top_p: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct ReasoningConfig {
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub summary: bool,
    #[serde(default)]
    pub budget_tokens: Option<u32>,
}

impl ReasoningConfig {
    pub fn is_enabled(&self) -> bool {
        self.effort
            .as_deref()
            .is_some_and(|effort| effort != "none")
            || self.summary
            || self.budget_tokens.is_some()
    }

    pub fn new(effort: Option<String>, summary: bool) -> Self {
        Self {
            effort,
            summary,
            budget_tokens: None,
        }
    }

    pub fn with_budget_tokens(mut self, budget_tokens: Option<u32>) -> Self {
        self.budget_tokens = budget_tokens;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct ModelLimits {
    pub max_input_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
}

impl ModelLimits {
    pub fn new(max_input_tokens: Option<u32>, max_output_tokens: Option<u32>) -> Self {
        Self {
            max_input_tokens,
            max_output_tokens,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct CacheHints {
    pub cache_instructions: bool,
    pub cache_context: bool,
    /// Provider-neutral routing namespace for requests that share a stable
    /// prompt prefix. Adapters decide whether and how to map it to provider
    /// wire fields.
    pub routing_key: Option<String>,
}

impl CacheHints {
    pub fn new(cache_instructions: bool, cache_context: bool) -> Self {
        Self {
            cache_instructions,
            cache_context,
            routing_key: None,
        }
    }

    pub fn with_routing_key(mut self, routing_key: impl Into<String>) -> Self {
        self.routing_key = Some(routing_key.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub enum ResponseFormat {
    #[default]
    Text,
    Json,
    JsonSchema {
        name: String,
        schema: serde_json::Value,
        strict: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ToolChoice {
    None,
    #[default]
    Auto,
    Required,
    Tool(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_model_controls_reject_unknown_fields() {
        for invalid in [
            serde_json::json!({"budget_token":1024}),
            serde_json::json!({"effort":"high","surprise":true}),
        ] {
            assert!(serde_json::from_value::<ReasoningConfig>(invalid).is_err());
        }
        assert!(
            serde_json::from_value::<SamplingConfig>(serde_json::json!({"top_p":0.5,"temp":0.1}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<ModelLimits>(serde_json::json!({"max_output_token":1}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<CacheHints>(serde_json::json!({"cache_instruction":true}))
                .is_err()
        );
        assert!(serde_json::from_value::<ResponseFormat>(serde_json::json!({"JsonSchema":{"name":"x","schema":{},"strict":true,"extra":true}})).is_err());
    }
}
