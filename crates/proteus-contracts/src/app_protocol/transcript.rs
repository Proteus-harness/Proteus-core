use crate::model_standard::MessagePhase;
use serde_json::Value;

/// A page of one immutable display projection. Positions address that revision only.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppTranscriptPage {
    pub revision: String,
    pub start: usize,
    pub total: usize,
    pub messages: Vec<AppTranscriptMessage>,
    pub base: Option<AppTranscriptBase>,
}

/// Older completed rows exist before this position in the named display revision.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppTranscriptCursor {
    pub revision: String,
    pub before: usize,
    pub base: Option<AppTranscriptBase>,
}

/// Prefix verified byte-for-byte against the previous display projection.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppTranscriptBase {
    pub revision: String,
    pub common_prefix: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppTranscriptMessage {
    pub message_id: Option<crate::domain::MessageId>,
    /// First recorded creation time in Unix milliseconds. Persisted rows use
    /// canonical journal time; unavailable source dates stay unknown.
    pub timestamp_ms: Option<i64>,
    pub phase: Option<MessagePhase>,
    pub role: String,
    pub text: String,
    pub images: Vec<crate::domain::ImageRef>,
    pub tool: Option<AppTranscriptTool>,
    pub subagent: Option<AppTranscriptSubagent>,
    /// Текст ещё стримится: сообщение — живой прогресс незавершённого хода
    /// (см. turn_progress), клиент продолжает дописывать в него дельты.
    pub streaming: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppTranscriptTool {
    pub call_id: String,
    pub name: String,
    pub args: Value,
    /// Аргументы после hooks и policy, прошедшие к исполнению. `args`
    /// сохраняет исходный запрос модели.
    pub effective_args: Option<Value>,
    pub status: String,
    pub result: Option<String>,
    /// Metadata результата как есть (`ToolResult.metadata`): core не знает
    /// конкретных tools, а клиенты по ней строят спец-рендеры (например,
    /// карточку субагента из результата `task`).
    pub metadata: Value,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppTranscriptSubagent {
    pub child_thread_id: String,
    pub role: String,
    pub description: Option<String>,
    pub status: String,
    pub iterations: Option<u32>,
    pub tools: Vec<AppTranscriptTool>,
}
