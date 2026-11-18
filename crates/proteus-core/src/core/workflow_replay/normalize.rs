use std::collections::HashMap;

use crate::{
    domain::{AgentOutput, CallId, HistoryCompactionReport, ToolCall, ToolResult},
    model_standard::{CanonicalMessage, CanonicalModelRequest, ContentPart},
};

#[cfg(test)]
#[test]
fn replay_compares_model_operation_as_a_request_fact() {
    let request =
        CanonicalModelRequest::new(crate::domain::ModelRef::new("fixture", "model"), vec![]);
    let compact = request
        .clone()
        .with_operation(crate::model_standard::ModelOperation::Compact);
    assert!(!requests_equal(&request, &compact, &HashMap::new()));
    assert_eq!(
        request_difference(&request, &compact, &HashMap::new()),
        "differing fields: operation"
    );
}

pub(super) fn requests_equal(
    actual: &CanonicalModelRequest,
    expected: &CanonicalModelRequest,
    call_ids: &HashMap<CallId, CallId>,
) -> bool {
    let mut actual = actual.clone();
    let mut expected = expected.clone();
    normalize_request(&mut actual, call_ids);
    normalize_request(&mut expected, &HashMap::new());
    actual == expected
}

/// Older preparation records carried an exact second copy of the bound material
/// in the request template. Replay compares model controls and uses the bound
/// messages for normalizing advisory estimates in both representations.
pub(super) fn preparation_requests_equal(
    actual: &crate::contracts::ContextPreparationInput,
    expected: &crate::contracts::ContextPreparationInput,
    ids: &HashMap<CallId, CallId>,
) -> bool {
    match (&actual.request, &expected.request) {
        (Some(a), Some(b)) => {
            if !(a.messages.is_empty() || a.messages == actual.messages)
                || !(b.messages.is_empty() || b.messages == expected.messages)
            {
                return requests_equal(a, b, ids);
            }
            let mut a = a.clone();
            let mut b = b.clone();
            a.messages.clear();
            b.messages.clear();
            normalize_estimate_metadata(&mut a.metadata, &actual.messages);
            normalize_estimate_metadata(&mut b.metadata, &expected.messages);
            rewrite_value_strings(&mut a.metadata, ids);
            a == b
        }
        (None, None) => true,
        _ => false,
    }
}

#[test]
fn preparation_replay_accepts_only_a_redundant_material_copy() {
    use crate::{
        contracts::ContextPreparationInput, domain::ModelRef, model_standard::MessageRole,
    };
    let request = CanonicalModelRequest::new(
        ModelRef::new("fixture", "model"),
        vec![CanonicalMessage::text(MessageRole::User, "source")],
    );
    let current = ContextPreparationInput::from_request(request.clone());
    let mut prior = ContextPreparationInput::new(request.messages.clone(), Some(request));
    let ids = HashMap::new();
    assert!(preparation_requests_equal(&current, &prior, &ids));
    prior.request.as_mut().unwrap().messages[0] =
        CanonicalMessage::text(MessageRole::User, "different");
    assert!(!preparation_requests_equal(&current, &prior, &ids));
    prior.request.as_mut().unwrap().messages.clear();
    prior.request.as_mut().unwrap().limits.max_output_tokens = Some(321);
    assert!(!preparation_requests_equal(&current, &prior, &ids));
}

pub(super) fn messages_equal(
    actual: &[CanonicalMessage],
    expected: &[CanonicalMessage],
    call_ids: &HashMap<CallId, CallId>,
) -> bool {
    let mut actual = actual.to_vec();
    let mut expected = expected.to_vec();
    normalize_messages(&mut actual, call_ids);
    normalize_messages(&mut expected, &HashMap::new());
    actual == expected
}

pub(super) fn calls_equal(
    actual: &ToolCall,
    expected: &ToolCall,
    call_ids: &HashMap<CallId, CallId>,
) -> bool {
    let mut actual = actual.clone();
    normalize_call(&mut actual, call_ids);
    actual == *expected
}

pub(super) fn results_equal(
    actual: &ToolResult,
    expected: &ToolResult,
    call_ids: &HashMap<CallId, CallId>,
) -> bool {
    let mut actual = actual.clone();
    let mut expected = expected.clone();
    normalize_result(&mut actual, call_ids);
    normalize_result(&mut expected, &HashMap::new());
    actual == expected
}

pub(super) fn outputs_equal(
    actual: &AgentOutput,
    expected: &AgentOutput,
    call_ids: &HashMap<CallId, CallId>,
) -> bool {
    let mut actual = actual.clone();
    let mut expected = expected.clone();
    normalize_output(&mut actual, call_ids);
    normalize_output(&mut expected, &HashMap::new());
    actual == expected
}

pub(super) fn changed_compactions_equal(
    actual: &[HistoryCompactionReport],
    expected: &[HistoryCompactionReport],
) -> bool {
    let actual = actual
        .iter()
        .filter(|report| report.changed)
        .collect::<Vec<_>>();
    let expected = expected
        .iter()
        .filter(|report| report.changed)
        .collect::<Vec<_>>();
    actual == expected
}

pub(super) fn request_difference(
    actual: &CanonicalModelRequest,
    expected: &CanonicalModelRequest,
    call_ids: &HashMap<CallId, CallId>,
) -> String {
    let mut actual = actual.clone();
    let mut expected = expected.clone();
    normalize_request(&mut actual, call_ids);
    normalize_request(&mut expected, &HashMap::new());
    let Ok(actual) = serde_json::to_value(actual) else {
        return "canonical request differs".to_owned();
    };
    let Ok(expected) = serde_json::to_value(expected) else {
        return "canonical request differs".to_owned();
    };
    let differing = [
        "operation",
        "model",
        "instructions",
        "messages",
        "tools",
        "tool_choice",
        "response_format",
        "sampling",
        "reasoning",
        "limits",
        "cache",
        "client_metadata",
        "metadata",
    ]
    .into_iter()
    .filter(|key| actual.get(*key) != expected.get(*key))
    .collect::<Vec<_>>();
    if differing.is_empty() {
        "canonical request differs".to_owned()
    } else {
        format!("differing fields: {}", differing.join(", "))
    }
}

pub(super) fn rewrite_result_call_ids(
    result: &mut ToolResult,
    expected_to_actual: &HashMap<CallId, CallId>,
) {
    if let Some(actual) = expected_to_actual.get(&result.call_id) {
        result.call_id = actual.clone();
    }
    rewrite_value_strings(&mut result.metadata, expected_to_actual);
}

fn normalize_request(request: &mut CanonicalModelRequest, call_ids: &HashMap<CallId, CallId>) {
    normalize_estimate_metadata(&mut request.metadata, &request.messages);
    normalize_messages(&mut request.messages, call_ids);
    rewrite_value_strings(&mut request.metadata, call_ids);
}

pub(super) fn estimate_metadata_equal(
    actual: &serde_json::Value,
    actual_messages: &[CanonicalMessage],
    expected: &serde_json::Value,
    expected_messages: &[CanonicalMessage],
) -> bool {
    let mut actual = actual.clone();
    let mut expected = expected.clone();
    normalize_estimate_metadata(&mut actual, actual_messages);
    normalize_estimate_metadata(&mut expected, expected_messages);
    actual == expected
}

fn normalize_estimate_metadata(metadata: &mut serde_json::Value, messages: &[CanonicalMessage]) {
    if let Some(mut estimate) = crate::model_standard::ModelInputEstimate::from_metadata(metadata) {
        // Advisory identities are derived from canonical position only for replay.
        // Never rewrite provider-owned opaque payloads.
        let ids: HashMap<_, _> = messages
            .iter()
            .flat_map(|message| &message.parts)
            .enumerate()
            .map(|(index, part)| (part.part_id, uuid::Uuid::from_u128(index as u128 + 1)))
            .collect();
        for part in &mut estimate.parts {
            if let Some(id) = ids.get(&part.part_id) {
                part.part_id = *id;
            }
        }
        estimate.parts.sort_by_key(|part| part.part_id);
        estimate.write_metadata(metadata);
    }
}

fn normalize_messages(messages: &mut [CanonicalMessage], call_ids: &HashMap<CallId, CallId>) {
    for message in messages {
        message.id = uuid::Uuid::nil();
        if let Some(call_id) = message.tool_call_id.as_mut()
            && let Some(expected) = call_ids.get(call_id)
        {
            *call_id = expected.clone();
        }
        rewrite_value_strings(&mut message.metadata, call_ids);
        for part in &mut message.parts {
            part.part_id = uuid::Uuid::nil();
            match &mut part.payload {
                ContentPart::ToolCall { call } => normalize_call(call, call_ids),
                ContentPart::ToolResult { result } => normalize_result(result, call_ids),
                ContentPart::Context { chunk } => {
                    rewrite_value_strings(&mut chunk.metadata, call_ids)
                }
                _ => {}
            }
        }
    }
}

fn normalize_call(call: &mut ToolCall, call_ids: &HashMap<CallId, CallId>) {
    if let Some(expected) = call_ids.get(&call.id) {
        call.id = expected.clone();
    }
    rewrite_value_strings(&mut call.args, call_ids);
}

fn normalize_result(result: &mut ToolResult, call_ids: &HashMap<CallId, CallId>) {
    if let Some(expected) = call_ids.get(&result.call_id) {
        result.call_id = expected.clone();
    }
    if let Some(metadata) = result.metadata.as_object_mut() {
        metadata.remove("duration_ms");
    }
    rewrite_value_strings(&mut result.metadata, call_ids);
    for content in &mut result.content {
        if let crate::domain::ToolContent::Json { value } = content {
            rewrite_value_strings(value, call_ids);
        }
    }
}

fn normalize_output(output: &mut AgentOutput, call_ids: &HashMap<CallId, CallId>) {
    rewrite_value_strings(&mut output.metadata, call_ids);
    if let Some(context) = output
        .metadata
        .as_object_mut()
        .and_then(|metadata| metadata.get_mut("context"))
        .and_then(serde_json::Value::as_object_mut)
    {
        // Workflow token estimates include serialized ToolResult metadata.
        // Replay necessarily measures a fresh duration_ms for each stubbed
        // invocation, so this derived estimate is nondeterministic too. Exact
        // model requests and normalized history remain the semantic checks.
        context.remove("token_estimate");
    }
}

fn rewrite_value_strings(value: &mut serde_json::Value, replacements: &HashMap<CallId, CallId>) {
    match value {
        serde_json::Value::String(text) => {
            if let Some(replacement) = replacements.get(text) {
                *text = replacement.clone();
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                rewrite_value_strings(value, replacements);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                rewrite_value_strings(value, replacements);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
#[test]
fn replay_normalizes_advisory_part_ids_without_changing_opaque_data() {
    use crate::model_standard::{MessageRole, ModelInputEstimate, ModelPartTokenEstimate};
    fn request(tokens: u32) -> CanonicalModelRequest {
        let message = CanonicalMessage::new(
            MessageRole::Assistant,
            vec![ContentPart::Opaque {
                format: "fixture/checkpoint".into(),
                data: serde_json::json!({"part_id":"provider-owned-unchanged"}),
            }],
        );
        let estimate = ModelInputEstimate {
            parts: vec![ModelPartTokenEstimate {
                part_id: message.parts[0].part_id,
                tokens,
            }],
        };
        let mut request = CanonicalModelRequest::new(
            crate::domain::ModelRef::new("fixture", "model"),
            vec![message],
        );
        estimate.write_metadata(&mut request.metadata);
        request
    }
    let a = request(13);
    let b = request(13);
    assert!(requests_equal(&a, &b, &HashMap::new()));
    assert!(!requests_equal(&a, &request(14), &HashMap::new()));
    let original = a.messages[0].parts[0].payload.clone();
    let mut normalized = a;
    normalize_request(&mut normalized, &HashMap::new());
    assert_eq!(normalized.messages[0].parts[0].payload, original);
}
