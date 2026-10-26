//! Resource regression for the real ordered preparation composition. The
//! reference below preserves the preceding material-copy path while both
//! versions call the same current identity validator; no timing assertion.
use super::*;
use crate::{domain::ModelRef, model_standard::MessageRole, test_alloc};

#[test]
fn composition_allocates_one_result_payload_without_cloning_bound_request() {
    const ROWS: usize = 512;
    const BYTES: usize = 8192;
    let messages: Vec<_> = (0..ROWS)
        .map(|_| CanonicalMessage::text(MessageRole::User, "x".repeat(BYTES)))
        .collect();
    let input = ContextPreparationInput::new(
        messages.clone(),
        Some(CanonicalModelRequest::new(
            ModelRef::new("fake", "fixture"),
            messages,
        )),
    );
    let previous = ContextPreparationOutput::unchanged(input.messages.clone());
    let candidate = previous.clone();
    let (expected, old) =
        test_alloc::measure(|| legacy_compose_outputs(&input, &previous, &candidate).unwrap());
    let (actual, new) =
        test_alloc::measure(|| compose_outputs(&input, &previous, &candidate).unwrap());
    assert_eq!(actual, expected);
    let payload = ROWS * BYTES;
    // One owned result is required. Identity indices and borrowed material
    // should fit in another payload's worth, independently of string content.
    assert!(
        new.bytes < 2 * payload,
        "composition allocation traffic {new:?} for payload {payload}"
    );
    assert!(
        old.bytes > 2 * payload,
        "negative control must detect material copies: {old:?}"
    );
    println!(
        "context_composition_allocations={}",
        serde_json::json!({
            "payload_bytes":payload,
            "legacy_allocated_bytes":old.bytes,
            "optimized_allocated_bytes":new.bytes,
            "legacy_calls":old.calls,
            "optimized_calls":new.calls,
            "scope":"same indexed validator; aggregate allocation traffic on this thread, not RSS"
        })
    );
}

fn legacy_compose_outputs(
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
    crate::contracts::validate_context_preparation_output(&material, candidate)?;
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
    crate::contracts::validate_context_preparation_output(input, &composed)?;
    Ok(composed)
}
