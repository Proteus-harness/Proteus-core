//! Rebind recorded call references without interpreting canonical opaque content.
use std::collections::HashMap;

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use crate::{
    contracts::HookResponse,
    domain::CallId,
    model_standard::{CanonicalMessage, ContentPart},
};

pub(super) fn rewrite_response(
    response: &HookResponse,
    ids: &HashMap<CallId, CallId>,
) -> Result<HookResponse> {
    let mut value = serde_json::to_value(response)?;
    let canonical = match response {
        HookResponse::ModelContext { messages, .. } => Some(("/messages", messages)),
        HookResponse::ContextPreparation { output } => Some(("/output/messages", &output.messages)),
        HookResponse::Continue
        | HookResponse::ToolSelection { .. }
        | HookResponse::ToolArguments { .. }
        | HookResponse::ContinueTurn { .. }
        | HookResponse::BlockTool { .. }
        | HookResponse::ToolOutput { .. } => None,
    };
    if let Some((path, messages)) = canonical {
        // Move the message array out while rewriting the surrounding fields.
        // A generic JSON walk must never enter provider-owned opaque payloads.
        let mut values = value
            .pointer_mut(path)
            .context("canonical response messages missing")?
            .take();
        rewrite_message_values(&mut values, messages, ids)?;
        rewrite_strings(&mut value, ids);
        *value
            .pointer_mut(path)
            .context("canonical response messages missing")? = values;
    } else {
        rewrite_strings(&mut value, ids);
    }
    Ok(serde_json::from_value(value)?)
}

pub(super) fn rewrite_messages(
    messages: &[CanonicalMessage],
    ids: &HashMap<CallId, CallId>,
) -> Result<Vec<CanonicalMessage>> {
    let mut values = serde_json::to_value(messages)?;
    rewrite_message_values(&mut values, messages, ids)?;
    Ok(serde_json::from_value(values)?)
}

fn rewrite_message_values(
    values: &mut Value,
    messages: &[CanonicalMessage],
    ids: &HashMap<CallId, CallId>,
) -> Result<()> {
    let values = values
        .as_array_mut()
        .context("canonical messages must be an array")?;
    ensure!(
        values.len() == messages.len(),
        "canonical message count changed during encoding"
    );
    for (value, message) in values.iter_mut().zip(messages) {
        let mut protected = Vec::new();
        for (index, part) in message.parts.iter().enumerate() {
            if matches!(part.payload, ContentPart::Opaque { .. }) {
                // Move, rather than clone, ciphertext and arbitrary data. Only
                // the typed canonical part defines this preservation boundary;
                // a user JSON object named `Opaque` is ordinary rewrite material.
                let payload = value
                    .get_mut("parts")
                    .and_then(Value::as_array_mut)
                    .and_then(|parts| parts.get_mut(index))
                    .and_then(|part| part.get_mut("payload"))
                    .context("canonical part payload missing")?
                    .take();
                protected.push((index, payload));
            }
        }
        rewrite_strings(value, ids);
        for (index, payload) in protected {
            value["parts"][index]["payload"] = payload;
        }
    }
    Ok(())
}

fn rewrite_strings(value: &mut Value, ids: &HashMap<CallId, CallId>) {
    match value {
        Value::String(text) => {
            if let Some(actual) = ids.get(text) {
                *text = actual.clone();
            }
        }
        Value::Array(values) => {
            for value in values {
                rewrite_strings(value, ids);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                rewrite_strings(value, ids);
            }
        }
        _ => {}
    }
}
