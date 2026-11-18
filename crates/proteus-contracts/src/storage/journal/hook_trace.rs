//! Lossless journal representation; live hook and analysis DTOs keep their wire format.
use crate::contracts::{HookEvent, HookInput, HookStep, HookTrace, OperationOwner};
use ring::digest::{SHA256, digest};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error, ser::Error as _};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

const ENCODING: &str = "shared_hook_messages_v1";
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MIN_SHARED_BYTES: usize = 4096;
const MAX_REFERENCES: usize = 4096;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Compact {
    encoding: String,
    trace: Value,
    unchanged_output: bool,
    messages: Vec<Value>,
    references: Vec<Reference>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    path: String,
    index: usize,
}

#[derive(Serialize)]
struct TraceFields<'a> {
    owner: &'a OperationOwner,
    input: &'a HookInput,
    steps: &'a [HookStep],
    output: Option<&'a HookEvent>,
}

fn event_paths(value: &Value, base: &str, paths: &mut Vec<String>) {
    let suffixes: &[&str] = match value.get("event").and_then(Value::as_str) {
        Some("turn_started" | "before_stop") => &["/history"],
        Some("before_model") => &["/request/messages"],
        Some("before_context") => &[
            "/input/messages",
            "/input/request/messages",
            "/output/messages",
        ],
        _ => &[],
    };
    paths.extend(suffixes.iter().map(|suffix| format!("{base}{suffix}")));
}

fn message_paths(trace: &Value) -> Vec<String> {
    let mut paths = vec!["/input/conversation/messages".into()];
    event_paths(&trace["input"]["event"], "/input/event", &mut paths);
    event_paths(&trace["output"], "/output", &mut paths);
    if let Some(steps) = trace["steps"].as_array() {
        for (index, step) in steps.iter().enumerate() {
            let response = &step["outcome"]["response"];
            let suffix = match response["action"].as_str() {
                Some("model_context") => Some("/messages"),
                Some("context_preparation") => Some("/output/messages"),
                _ => None,
            };
            if let Some(suffix) = suffix {
                paths.push(format!("/steps/{index}/outcome/response{suffix}"));
            }
        }
    }
    paths
}

pub(super) fn serialize<S: Serializer>(
    trace: &HookTrace,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let unchanged_output = trace.output.as_ref() == Some(&trace.input.event);
    let mut value = serde_json::to_value(TraceFields {
        owner: &trace.owner,
        input: &trace.input,
        steps: &trace.steps,
        output: trace.output.as_ref().filter(|_| !unchanged_output),
    })
    .map_err(S::Error::custom)?;
    let mut expanded_bytes = serde_json::to_vec(&value).map_err(S::Error::custom)?.len();
    if unchanged_output {
        expanded_bytes = expanded_bytes.saturating_sub(4).saturating_add(
            serde_json::to_vec(&value["input"]["event"])
                .map_err(S::Error::custom)?
                .len(),
        );
    }
    if expanded_bytes > MAX_BYTES {
        return Err(S::Error::custom("expanded hook trace exceeds maximum"));
    }
    let mut groups: HashMap<Vec<u8>, Vec<String>> = HashMap::new();
    for path in message_paths(&value) {
        let Some(array) = value.pointer(&path).filter(|value| value.is_array()) else {
            continue;
        };
        let bytes = serde_json::to_vec(array).map_err(S::Error::custom)?;
        if bytes.len() < MIN_SHARED_BYTES {
            continue;
        }
        let key = digest(&SHA256, &bytes).as_ref().to_vec();
        let paths = groups.entry(key).or_default();
        // Exact equality keeps the transformation lossless even on a digest collision.
        if paths
            .first()
            .is_none_or(|first| value.pointer(first) == Some(array))
        {
            paths.push(path);
        }
    }
    let mut messages = Vec::new();
    let mut references = Vec::new();
    let mut groups: Vec<_> = groups
        .into_values()
        .filter(|paths| paths.len() > 1)
        .collect();
    groups.sort_by(|left, right| left[0].cmp(&right[0]));
    for paths in groups {
        let index = messages.len();
        messages.push(
            value
                .pointer_mut(&paths[0])
                .expect("collected array path")
                .take(),
        );
        for path in paths {
            value
                .pointer_mut(&path)
                .expect("collected array path")
                .take();
            references.push(Reference { path, index });
        }
    }
    if references.len() > MAX_REFERENCES {
        return trace.serialize(serializer);
    }
    if !unchanged_output && references.is_empty() {
        return value.serialize(serializer);
    }
    let compact = Compact {
        encoding: ENCODING.into(),
        trace: value,
        unchanged_output,
        messages,
        references,
    };
    // Small events can cost more as an envelope. Near the payload limit the
    // canonical representation must remain available instead of emitting an
    // encoding that the bounded storage reader would reject.
    let compact_bytes = serde_json::to_vec(&compact).map_err(S::Error::custom)?;
    if compact_bytes.len() >= expanded_bytes {
        return trace.serialize(serializer);
    }
    compact.serialize(serializer)
}

pub(super) fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<HookTrace, D::Error> {
    let value = Value::deserialize(deserializer)?;
    if value.get("encoding").is_none() {
        return serde_json::from_value(value).map_err(D::Error::custom);
    }
    let mut compact: Compact = serde_json::from_value(value).map_err(D::Error::custom)?;
    if compact.encoding != ENCODING
        || compact.references.len() > MAX_REFERENCES
        || compact.messages.len() > compact.references.len()
    {
        return Err(D::Error::custom(
            "unsupported or oversized hook trace encoding",
        ));
    }
    let allowed: HashSet<_> = message_paths(&compact.trace).into_iter().collect();
    let mut restored = HashSet::new();
    let mut bytes = serde_json::to_vec(&compact.trace)
        .map_err(D::Error::custom)?
        .len();
    let sizes: Vec<_> = compact
        .messages
        .iter()
        .map(|messages| {
            if !messages.is_array() {
                return Err(D::Error::custom("hook message pool contains a non-array"));
            }
            serde_json::to_vec(messages)
                .map(|bytes| bytes.len())
                .map_err(D::Error::custom)
        })
        .collect::<Result<_, _>>()?;
    for reference in compact.references {
        if !allowed.contains(&reference.path) || !restored.insert(reference.path.clone()) {
            return Err(D::Error::custom(
                "hook message reference has an invalid or repeated path",
            ));
        }
        let messages = compact
            .messages
            .get(reference.index)
            .ok_or_else(|| D::Error::custom("hook message reference is out of range"))?;
        bytes = bytes
            .saturating_sub(4)
            .saturating_add(sizes[reference.index]);
        if bytes > MAX_BYTES {
            return Err(D::Error::custom("expanded hook trace exceeds maximum"));
        }
        let target = compact
            .trace
            .pointer_mut(&reference.path)
            .filter(|target| target.is_null())
            .ok_or_else(|| D::Error::custom("hook message reference overwrites existing data"))?;
        *target = messages.clone();
    }
    if compact.unchanged_output {
        if !compact.trace.get("output").is_some_and(Value::is_null) {
            return Err(D::Error::custom("unchanged hook output has existing data"));
        }
        let event = &compact.trace["input"]["event"];
        bytes = bytes
            .saturating_sub(4)
            .saturating_add(serde_json::to_vec(event).map_err(D::Error::custom)?.len());
        if bytes > MAX_BYTES {
            return Err(D::Error::custom("expanded hook trace exceeds maximum"));
        }
        compact.trace["output"] = event.clone();
    }
    serde_json::from_value(compact.trace).map_err(D::Error::custom)
}

#[cfg(test)]
mod tests;
