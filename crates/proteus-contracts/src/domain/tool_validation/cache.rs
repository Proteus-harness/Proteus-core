//! One schema slot owned by a registered tool, not an authorization cache.
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::{ToolCall, ToolSpec, compile_schema, tool_call_args, validate_args};

#[derive(Default)]
pub(crate) struct ToolSchemaCache {
    current: Mutex<Option<CompiledSchema>>,
}

struct CompiledSchema {
    schema: Value,
    validator: Arc<jsonschema::Validator>,
}

impl ToolSchemaCache {
    pub(crate) fn validator(&self, spec: &ToolSpec) -> Result<Arc<jsonschema::Validator>, String> {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(compiled) = current.as_ref()
            && same_schema(&compiled.schema, &spec.input_schema)
        {
            return Ok(Arc::clone(&compiled.validator));
        }
        // Tool::spec may change. Never freeze it, key by name alone, or fall
        // back to a previous schema when compilation of the current one fails.
        // The lock coalesces concurrent compilation for this one tool only.
        *current = None;
        let validator = Arc::new(compile_schema(spec)?);
        *current = Some(CompiledSchema {
            schema: spec.input_schema.clone(),
            validator: Arc::clone(&validator),
        });
        Ok(validator)
    }

    pub(crate) fn validate_call_args(&self, call: &ToolCall, spec: &ToolSpec) -> Option<String> {
        // Raw provider arguments remain authoritative and are parsed afresh.
        // In particular, a cache hit must not reuse a previous argument result.
        let args = match tool_call_args(call) {
            Ok(args) => args,
            Err(error) => return Some(error),
        };
        let validator = match self.validator(spec) {
            Ok(validator) => validator,
            Err(error) => return Some(error),
        };
        validate_args(call, &validator, &args)
    }
}

fn same_schema(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => {
            // serde_json/preserve_order may be enabled by another crate.
            // Value equality ignores map order, but the compiler's first-error
            // diagnostic can depend on traversal order, including nested maps.
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|((lk, lv), (rk, rv))| lk == rk && same_schema(lv, rv))
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(l, r)| same_schema(l, r))
        }
        // Equal floats can have different diagnostic spellings (e.g. -0.0).
        // Compare only these small scalar renderings, not whole encoded schemas.
        (Value::Number(left), Value::Number(right)) => left.to_string() == right.to_string(),
        _ => left == right,
    }
}

#[cfg(test)]
mod tests;
