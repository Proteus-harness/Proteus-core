use std::{
    borrow::Cow,
    fmt,
    io::{self, Write},
};

use serde::{Deserialize, Serialize};

use crate::domain::{HostedToolConfig, HostedToolKind, ids::CallId};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ToolCall {
    pub id: CallId,
    pub name: String,
    pub args: serde_json::Value,
    pub surface: ToolCallSurface,
    /// Original provider argument payload when exact replay matters (notably
    /// malformed Responses function-call JSON). Ordinary callers leave it out.
    pub raw_arguments: Option<String>,
}

impl ToolCall {
    pub fn new(id: impl Into<CallId>, name: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            args,
            surface: ToolCallSurface::default(),
            raw_arguments: None,
        }
    }

    pub fn with_surface(mut self, surface: ToolCallSurface) -> Self {
        self.surface = surface;
        self
    }

    pub fn with_raw_arguments(mut self, raw_arguments: impl Into<String>) -> Self {
        self.raw_arguments = Some(raw_arguments.into());
        self
    }
}

/// Canonical pre-invocation resolution for a local tool call. A durable
/// recorder writes this before the target tool can perform a side effect.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ToolCallResolution {
    Allowed,
    Approved,
    ApprovalDenied { reason: String },
    PolicyDenied { reason: String },
    HookBlocked { reason: String },
    ValidationFailed { reason: String },
    Unsupported { reason: String },
}

impl ToolCallResolution {
    pub fn permits_side_effect(&self) -> bool {
        matches!(self, Self::Allowed | Self::Approved)
    }

    pub fn requested_approval(&self) -> bool {
        matches!(self, Self::Approved | Self::ApprovalDenied { .. })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolCallSurface {
    #[default]
    Function,
    Freeform,
}

impl ToolCallSurface {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Freeform => "freeform",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ToolResult {
    pub call_id: CallId,
    pub ok: bool,
    pub output: String,
    pub content: Vec<ToolContent>,
    pub error: Option<String>,
    pub metadata: serde_json::Value,
}

impl ToolResult {
    /// Успешный результат с текстовым выводом.
    pub fn ok(call_id: CallId, output: impl Into<String>) -> Self {
        Self {
            call_id,
            ok: true,
            output: output.into(),
            content: Vec::new(),
            error: None,
            metadata: serde_json::Value::Null,
        }
    }

    /// Результат-ошибка.
    pub fn error(call_id: CallId, error: impl Into<String>) -> Self {
        Self {
            call_id,
            ok: false,
            output: String::new(),
            content: Vec::new(),
            error: Some(error.into()),
            metadata: serde_json::Value::Null,
        }
    }

    /// Полный конструктор со всеми полями.
    pub fn new(
        call_id: CallId,
        ok: bool,
        output: String,
        content: Vec<ToolContent>,
        error: Option<String>,
        metadata: serde_json::Value,
    ) -> Self {
        Self {
            call_id,
            ok,
            output,
            content,
            error,
            metadata,
        }
    }

    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn with_content(mut self, content: Vec<ToolContent>) -> Self {
        self.content = content;
        self
    }

    /// Text representation for model/UI boundaries where an empty result would
    /// otherwise disappear.
    pub fn text_or_status(&self) -> String {
        if let Some(text) = self.direct_text() {
            return text.to_owned();
        }
        let items: Vec<_> = self
            .display_items()
            .map(|item| match item {
                ToolDisplayItem::Text(text) => Cow::Borrowed(text),
                item => Cow::Owned(item.to_string()),
            })
            .collect();
        if items.is_empty() {
            self.empty_status().to_owned()
        } else {
            items.join("\n")
        }
    }

    /// Stream the same text to a writer, propagating a byte limit or other
    /// write error before formatting any remaining content.
    pub fn write_text_or_status(&self, mut writer: impl Write) -> io::Result<()> {
        if let Some(text) = self.direct_text() {
            return writer.write_all(text.as_bytes());
        }
        let mut items = self.display_items();
        let Some(first) = items.next() else {
            return writer.write_all(self.empty_status().as_bytes());
        };
        write!(writer, "{first}")?;
        for item in items {
            write!(writer, "\n{item}")?;
        }
        Ok(())
    }

    fn direct_text(&self) -> Option<&str> {
        self.error
            .as_deref()
            .filter(|text| !text.is_empty())
            .or_else(|| (!self.output.is_empty()).then_some(self.output.as_str()))
    }

    fn display_items(&self) -> impl Iterator<Item = ToolDisplayItem<'_>> {
        self.content.iter().filter_map(|content| match content {
            ToolContent::Text { text } if !text.is_empty() => Some(ToolDisplayItem::Text(text)),
            ToolContent::Json { value } => Some(ToolDisplayItem::Json(value)),
            ToolContent::Image { mime_type, .. } => {
                Some(ToolDisplayItem::Media("image", mime_type))
            }
            ToolContent::Binary { mime_type, .. } => {
                Some(ToolDisplayItem::Media("binary", mime_type))
            }
            _ => None,
        })
    }

    fn empty_status(&self) -> &'static str {
        if self.ok {
            "(no output)"
        } else {
            "(tool failed without an error message)"
        }
    }
}

enum ToolDisplayItem<'a> {
    Text(&'a str),
    Json(&'a serde_json::Value),
    Media(&'static str, &'a str),
}

impl fmt::Display for ToolDisplayItem<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => formatter.write_str(text),
            Self::Json(value) => fmt::Display::fmt(value, formatter),
            Self::Media(kind, mime_type) => write!(formatter, "[{kind} tool content: {mime_type}]"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolContent {
    Text { text: String },
    Json { value: serde_json::Value },
    Image { mime_type: String, data: String },
    Binary { mime_type: String, data: String },
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn tool_spec_requires_explicit_parallel_permission_on_the_wire() {
        let spec = ToolSpec::new("probe", "probe", json!({}), ToolSafety::ReadOnly);
        let mut value = serde_json::to_value(&spec).unwrap();
        assert_eq!(value["supports_parallel_tool_calls"], false);
        value
            .as_object_mut()
            .unwrap()
            .remove("supports_parallel_tool_calls");
        assert!(
            serde_json::from_value::<ToolSpec>(value.clone())
                .unwrap_err()
                .to_string()
                .contains("supports_parallel_tool_calls")
        );
        value["supports_parallel_tool_calls"] = json!("true");
        assert!(serde_json::from_value::<ToolSpec>(value).is_err());
        let command = spec.with_parallel_tool_calls(true);
        assert_eq!(
            serde_json::from_value::<ToolSpec>(serde_json::to_value(&command).unwrap()).unwrap(),
            command
        );
    }

    #[test]
    fn successful_empty_tool_result_has_status_text() {
        let result = ToolResult::ok("call-1".to_owned(), "");

        assert_eq!(result.text_or_status(), "(no output)");
    }

    #[test]
    fn empty_output_uses_structured_text_content() {
        let result = ToolResult::ok("call-1".to_owned(), "").with_content(vec![
            ToolContent::Text {
                text: "structured text".to_owned(),
            },
            ToolContent::Json {
                value: json!({"ok": true}),
            },
        ]);

        assert_eq!(result.text_or_status(), "structured text\n{\"ok\":true}");
    }

    #[test]
    fn tool_call_requires_the_complete_canonical_shape() {
        let incomplete = serde_json::from_value::<ToolCall>(json!({
            "id": "call-1",
            "name": "read_file",
            "args": { "path": "README.md" }
        }));
        assert!(incomplete.is_err());

        let canonical = ToolCall::new(
            "call-1",
            "read_file",
            json!({
                "path": "README.md"
            }),
        );
        assert_eq!(
            serde_json::to_value(&canonical)
                .expect("serialize canonical ToolCall")
                .get("raw_arguments"),
            Some(&serde_json::Value::Null)
        );

        let malformed =
            ToolCall::new("call-2", "read_file", json!("{bad")).with_raw_arguments("{bad");
        let roundtrip: ToolCall = serde_json::from_value(
            serde_json::to_value(&malformed).expect("serialize raw ToolCall"),
        )
        .expect("deserialize raw ToolCall");
        assert_eq!(roundtrip.raw_arguments.as_deref(), Some("{bad"));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub surface: ToolSurface,
    pub safety: ToolSafety,
    /// Execution scheduling permission, independent of policy/safety. False
    /// requires exclusive execution within a workflow tool-call sequence.
    pub supports_parallel_tool_calls: bool,
    pub timeout_ms: Option<u64>,
    pub metadata: serde_json::Value,
}

impl ToolSpec {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
        safety: ToolSafety,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            surface: ToolSurface::default(),
            safety,
            supports_parallel_tool_calls: false,
            timeout_ms: None,
            metadata: serde_json::Value::Null,
        }
    }

    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    pub fn with_parallel_tool_calls(mut self, supports: bool) -> Self {
        self.supports_parallel_tool_calls = supports;
        self
    }

    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn with_surface(mut self, surface: ToolSurface) -> Self {
        self.surface = surface;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ToolSurface {
    Function {
        strict: bool,
        output_schema: Option<serde_json::Value>,
    },
    /// Function handled by the workflow. This declaration grants no host
    /// execution authority; delegated operations still use registered tools.
    WorkflowFunction {
        strict: bool,
        output_schema: Option<serde_json::Value>,
    },
    Freeform {
        format: FreeformToolFormat,
    },
    /// Tool executed by the selected model provider inside the model request.
    /// It remains a normal `ToolSpec` for registry/policy/exposure purposes,
    /// but never becomes a client-executed `ToolCall`.
    ProviderHosted {
        config: HostedToolConfig,
    },
}

impl ToolSurface {
    pub fn function() -> Self {
        Self::default()
    }

    pub fn strict_function() -> Self {
        Self::Function {
            strict: true,
            output_schema: None,
        }
    }

    pub const fn workflow_function() -> Self {
        Self::WorkflowFunction {
            strict: false,
            output_schema: None,
        }
    }

    pub fn freeform_lark(definition: impl Into<String>) -> Self {
        Self::Freeform {
            format: FreeformToolFormat::lark(definition),
        }
    }

    pub const fn provider_hosted(config: HostedToolConfig) -> Self {
        Self::ProviderHosted { config }
    }

    pub const fn hosted_kind(&self) -> Option<HostedToolKind> {
        match self {
            Self::ProviderHosted { config } => Some(config.kind()),
            _ => None,
        }
    }

    /// Local call surface expected back from the model. Provider-hosted tools
    /// return activity items instead, so they deliberately have no value here.
    pub const fn call_surface(&self) -> Option<ToolCallSurface> {
        match self {
            Self::Function { .. } | Self::WorkflowFunction { .. } => {
                Some(ToolCallSurface::Function)
            }
            Self::Freeform { .. } => Some(ToolCallSurface::Freeform),
            Self::ProviderHosted { .. } => None,
        }
    }
}

impl Default for ToolSurface {
    fn default() -> Self {
        Self::Function {
            strict: false,
            output_schema: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct FreeformToolFormat {
    #[serde(rename = "type")]
    pub kind: String,
    pub syntax: String,
    pub definition: String,
}

impl FreeformToolFormat {
    pub fn lark(definition: impl Into<String>) -> Self {
        Self {
            kind: "grammar".to_owned(),
            syntax: "lark".to_owned(),
            definition: definition.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolSafety {
    ReadOnly,
    WritesFiles,
    RunsCommands,
    Network,
    Dangerous,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Plan,
    #[default]
    Normal,
    /// Automatically approve calls unless the selected policy denies them.
    /// This does not grant OS or sandbox permissions.
    FullAccess,
    /// Legacy mode: read/write tools only; commands and network stay denied.
    Auto,
}

impl PermissionMode {
    /// Modes offered for a new selection. Auto remains readable and executable
    /// for persisted configurations and clients using the legacy value.
    pub const SELECTABLE: [Self; 3] = [Self::Plan, Self::Normal, Self::FullAccess];

    /// Canonical value shared by config, API and child-process CLI arguments.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Normal => "normal",
            Self::FullAccess => "full_access",
            Self::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub enum PolicyDecision {
    Allow,
    Ask { reason: String },
    Deny { reason: String },
}
