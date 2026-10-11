use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    domain::{CompactionUserMessageReplacement, HistoryCompactionReport},
    model_standard::{CanonicalMessage, CanonicalModelRequest},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ContextPreparationInput {
    pub messages: Vec<CanonicalMessage>,
    /// Optional model controls for strategies which need auxiliary generation.
    #[serde(deserialize_with = "Option::deserialize")]
    pub request: Option<CanonicalModelRequest>,
    pub token_estimate: Option<u32>,
    /// Сырой потолок контекстного окна модели. Способ использования определяет
    /// выбранная стратегия подготовки. `None` — если окно неизвестно.
    pub window_tokens: Option<u32>,
    pub reason: Option<String>,
}

impl ContextPreparationInput {
    pub fn new(messages: Vec<CanonicalMessage>, request: Option<CanonicalModelRequest>) -> Self {
        Self {
            messages,
            request,
            token_estimate: None,
            window_tokens: None,
            reason: None,
        }
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    pub fn with_token_estimate(mut self, token_estimate: Option<u32>) -> Self {
        self.token_estimate = token_estimate;
        self
    }

    pub fn with_window_tokens(mut self, window_tokens: Option<u32>) -> Self {
        self.window_tokens = window_tokens;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ContextPreparationOutput {
    pub messages: Vec<CanonicalMessage>,
    /// Explicit replacements of accepted user inputs. Contents must never be
    /// rewritten under an existing message identity.
    pub user_message_replacements: Vec<CompactionUserMessageReplacement>,
    pub changed: bool,
    pub summary: Option<String>,
    pub token_estimate: Option<u32>,
    /// Module's estimate for the original input, if it computed one itself.
    /// Otherwise the report retains the estimate supplied in ContextPreparationInput.
    pub original_token_estimate: Option<u32>,
    /// Token-based trigger, if applicable to this preparation strategy.
    pub trigger_tokens: Option<u32>,
    /// Descriptive module-defined labels; consumers must not dispatch on them.
    pub summary_source: Option<String>,
    pub skipped_reason: Option<String>,
    /// Opaque diagnostics. Never overrides the typed result or message counts.
    pub metadata: serde_json::Value,
}

impl ContextPreparationOutput {
    pub fn changed(messages: Vec<CanonicalMessage>, summary: impl Into<Option<String>>) -> Self {
        Self {
            messages,
            changed: true,
            user_message_replacements: Vec::new(),
            summary: summary.into(),
            token_estimate: None,
            original_token_estimate: None,
            trigger_tokens: None,
            summary_source: None,
            skipped_reason: None,
            metadata: serde_json::Value::Null,
        }
    }

    pub fn unchanged(messages: Vec<CanonicalMessage>) -> Self {
        Self {
            messages,
            changed: false,
            user_message_replacements: Vec::new(),
            summary: None,
            token_estimate: None,
            original_token_estimate: None,
            trigger_tokens: None,
            summary_source: None,
            skipped_reason: None,
            metadata: serde_json::Value::Null,
        }
    }
}

impl HistoryCompactionReport {
    pub fn from_context_preparation_output(
        input: &ContextPreparationInput,
        output: &ContextPreparationOutput,
    ) -> Self {
        Self {
            changed: output.changed,
            user_message_replacements: output.user_message_replacements.clone(),
            reason: input.reason.clone(),
            input_messages: input.messages.len(),
            output_messages: output.messages.len(),
            original_token_estimate: output.original_token_estimate.or(input.token_estimate),
            output_token_estimate: output.token_estimate,
            trigger_tokens: output.trigger_tokens,
            summary_source: output.summary_source.clone(),
            skipped_reason: output.skipped_reason.clone(),
            summary: output.summary.clone(),
            metadata: output.metadata.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextPreparationTarget {
    pub execution_id: crate::domain::ExecutionId,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ContextPreparationResult {
    pub output: ContextPreparationOutput,
    #[serde(deserialize_with = "Option::deserialize")]
    pub target: Option<ContextPreparationTarget>,
}

/// Host-owned orchestration of the ordered preparation hook chain.
#[async_trait]
pub trait ContextPreparation: Send + Sync {
    async fn prepare(&self, input: ContextPreparationInput) -> Result<ContextPreparationResult>;
}

pub(crate) fn compose_outputs(
    input: &ContextPreparationInput,
    previous: &ContextPreparationOutput,
    candidate: &ContextPreparationOutput,
) -> Result<ContextPreparationOutput> {
    for message in &candidate.messages {
        if let Some(old) = previous.messages.iter().find(|old| old.id == message.id) {
            anyhow::ensure!(
                old == message,
                "context preparation changes a preceding hook message identity"
            );
        }
    }
    let mut material = input.clone();
    material.messages = previous.messages.clone();
    if candidate.changed {
        for message in &input.messages {
            if !material.messages.iter().any(|old| old.id == message.id) {
                material.messages.push(message.clone());
            }
        }
    }
    super::validate_context_preparation_output(&material, candidate)?;
    let mut composed = candidate.clone();
    composed.changed |= previous.changed;
    if !candidate.changed && previous.changed {
        composed = previous.clone();
    }
    let roots = previous
        .user_message_replacements
        .iter()
        .map(|mapping| (mapping.replacement_message_id, mapping.source_message_id))
        .collect::<std::collections::HashMap<_, _>>();
    composed.user_message_replacements = previous
        .user_message_replacements
        .iter()
        .filter(|mapping| {
            candidate
                .messages
                .iter()
                .any(|message| message.id == mapping.replacement_message_id)
        })
        .cloned()
        .collect();
    for replacement in &candidate.user_message_replacements {
        let root = roots
            .get(&replacement.source_message_id)
            .copied()
            .or_else(|| {
                input
                    .messages
                    .iter()
                    .find(|message| message.id == replacement.source_message_id)
                    .map(|message| message.id)
            });
        if let Some(source_message_id) = root {
            composed.user_message_replacements.push(
                crate::domain::CompactionUserMessageReplacement {
                    source_message_id,
                    replacement_message_id: replacement.replacement_message_id,
                },
            );
        }
    }
    if previous.changed {
        composed.original_token_estimate =
            previous.original_token_estimate.or(input.token_estimate);
    }
    super::validate_context_preparation_output(input, &composed)?;
    Ok(composed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        contracts::{HookEvent, HookResponse, apply_hook_response},
        domain::{ModelRef, new_execution_id},
        model_standard::MessageRole,
    };
    use serde_json::json;

    fn input() -> ContextPreparationInput {
        ContextPreparationInput::new(
            vec![CanonicalMessage::text(MessageRole::User, "source")],
            None,
        )
    }

    #[test]
    fn model_free_preparation_is_explicit_and_rejects_old_slot_payload() {
        let input = input();
        let value = serde_json::to_value(&input).unwrap();
        assert!(value["request"].is_null());
        assert_eq!(
            serde_json::from_value::<ContextPreparationInput>(value.clone()).unwrap(),
            input
        );
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove("request");
        assert!(serde_json::from_value::<ContextPreparationInput>(missing).is_err());
        for field in ["task", "config"] {
            let mut invalid = value.clone();
            invalid[field] = json!({});
            assert!(serde_json::from_value::<ContextPreparationInput>(invalid).is_err());
        }
    }

    #[test]
    fn request_template_retains_model_controls_without_owning_material() {
        let mut input = input();
        let mut request = CanonicalModelRequest::new(ModelRef::new("fake", "model"), vec![]);
        request.cache.routing_key = Some("routing".into());
        request.limits.max_output_tokens = Some(321);
        input.request = Some(request);
        let decoded: ContextPreparationInput =
            serde_json::from_value(serde_json::to_value(&input).unwrap()).unwrap();
        assert_eq!(decoded, input);
        assert_eq!(decoded.messages.len(), 1);
        assert!(decoded.request.unwrap().messages.is_empty());
    }

    #[test]
    fn hook_preparation_cannot_replace_its_bound_target_or_source_identity() {
        let input = input();
        let output = ContextPreparationOutput::unchanged(input.messages.clone());
        let target = Some(ContextPreparationTarget {
            execution_id: new_execution_id(),
            revision: 7,
        });
        let event = HookEvent::BeforeContext {
            input: input.clone(),
            output: output.clone(),
            target: target.clone(),
        };
        let mut invalid = output.clone();
        invalid.messages[0] = CanonicalMessage::text(MessageRole::User, "rewritten");
        invalid.messages[0].id = input.messages[0].id;
        invalid.changed = true;
        assert!(
            apply_hook_response(
                &event,
                &HookResponse::ContextPreparation { output: invalid }
            )
            .is_err()
        );
        let fresh = CanonicalMessage::text(MessageRole::User, "first hook summary");
        let previous =
            ContextPreparationOutput::changed(vec![fresh.clone(), input.messages[0].clone()], None);
        let mut rewritten = previous.clone();
        rewritten.messages[0].parts[0].payload = crate::model_standard::ContentPart::Text {
            text: "rewritten by second hook".into(),
        };
        let intermediate = HookEvent::BeforeContext {
            input: input.clone(),
            output: previous,
            target: target.clone(),
        };
        assert!(
            apply_hook_response(
                &intermediate,
                &HookResponse::ContextPreparation { output: rewritten }
            )
            .is_err()
        );
        let accepted =
            apply_hook_response(&event, &HookResponse::ContextPreparation { output }).unwrap();
        assert_eq!(accepted, event);
        let value = json!({"action":"context_preparation", "output":ContextPreparationOutput::unchanged(input.messages),"target":target});
        assert!(serde_json::from_value::<HookResponse>(value).is_err());
    }

    #[test]
    fn ordered_preparation_composes_user_derivations_back_to_root() {
        use crate::model_standard::{CanonicalPart, ContentPart, PartProvenance, PartScope};
        let input = input().with_token_estimate(Some(100));
        let derived = |text: &str| {
            CanonicalMessage::from_parts(
                MessageRole::User,
                vec![CanonicalPart::new(
                    PartProvenance::Compactor,
                    PartScope::Conversation,
                    ContentPart::Text { text: text.into() },
                )],
            )
        };
        let first = derived("first summary");
        let second = derived("second summary");
        let mut first_output = ContextPreparationOutput::changed(vec![first.clone()], None);
        first_output.original_token_estimate = Some(123);
        first_output
            .user_message_replacements
            .push(CompactionUserMessageReplacement {
                source_message_id: input.messages[0].id,
                replacement_message_id: first.id,
            });
        let original = HookEvent::BeforeContext {
            input: input.clone(),
            output: ContextPreparationOutput::unchanged(input.messages.clone()),
            target: None,
        };
        let after_first = apply_hook_response(
            &original,
            &HookResponse::ContextPreparation {
                output: first_output,
            },
        )
        .unwrap();
        let mut second_output = ContextPreparationOutput::changed(vec![second.clone()], None);
        second_output.original_token_estimate = Some(50);
        second_output
            .user_message_replacements
            .push(CompactionUserMessageReplacement {
                source_message_id: first.id,
                replacement_message_id: second.id,
            });
        let after_second = apply_hook_response(
            &after_first,
            &HookResponse::ContextPreparation {
                output: second_output,
            },
        )
        .unwrap();
        let HookEvent::BeforeContext { output, .. } = &after_second else {
            panic!()
        };
        assert_eq!(output.original_token_estimate, Some(123));
        assert_eq!(
            output.user_message_replacements,
            vec![CompactionUserMessageReplacement {
                source_message_id: input.messages[0].id,
                replacement_message_id: second.id
            }]
        );
        let final_event = apply_hook_response(
            &after_second,
            &HookResponse::ContextPreparation {
                output: ContextPreparationOutput::unchanged(vec![second]),
            },
        )
        .unwrap();
        assert_eq!(final_event, after_second);
    }

    #[test]
    fn typed_diagnostics_are_strict_and_metadata_does_not_override_report() {
        let input = input().with_token_estimate(Some(64));
        let mut output = ContextPreparationOutput::unchanged(input.messages.clone());
        output.token_estimate = Some(12);
        output.metadata = json!({"input_messages":999,"trigger_tokens":999});
        let report = HistoryCompactionReport::from_context_preparation_output(&input, &output);
        assert_eq!(report.input_messages, 1);
        assert_eq!(report.output_token_estimate, Some(12));
        assert_eq!(report.original_token_estimate, Some(64));
        assert_eq!(report.trigger_tokens, None);
        let valid = serde_json::to_value(HookResponse::ContextPreparation { output }).unwrap();
        for (field, value) in [
            ("original_token_estimate", json!(-1)),
            ("trigger_tokens", json!("80")),
            ("summary_source", json!([])),
            ("skipped_reason", json!(false)),
        ] {
            let mut invalid = valid.clone();
            invalid["output"][field] = value;
            assert!(serde_json::from_value::<HookResponse>(invalid).is_err());
        }
    }
}
