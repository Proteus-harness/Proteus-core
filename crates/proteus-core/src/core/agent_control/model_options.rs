//! Model-facing options are revalidated by the process control plane.
use serde_json::{Map, Value};

use crate::domain::ToolCall;

pub(super) fn parse(call: &ToolCall) -> Result<Map<String, Value>, String> {
    let mut options = Map::new();
    for key in ["model", "effort"] {
        if let Some(value) = call.args.get(key) {
            let value = value
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| format!("{} arg '{key}' must be a nonempty string", call.name))?;
            options.insert(key.to_owned(), Value::String(value.to_owned()));
        }
    }
    Ok(options)
}
