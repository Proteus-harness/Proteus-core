use crate::{
    contracts::{ApprovalCacheScope, UserInputResponse},
    domain::{MessageId, ModelRef, PermissionMode, ReasoningConfig, RunOptions},
};
use serde::{Deserialize, Serialize};

/// Команды от клиента к ядру через stdin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum StdioRequest {
    CommandCatalog {
        id: Option<String>,
    },
    ExecuteCommand {
        id: Option<String>,
        text: String,
    },
    Send {
        id: Option<String>,
        text: String,
        #[serde(default)]
        images: Vec<crate::domain::ImageAttachment>,
        #[serde(default)]
        options: RunOptions,
    },
    EditQueuedMessage {
        id: Option<String>,
        message_id: MessageId,
        text: String,
    },
    DeleteQueuedMessage {
        id: Option<String>,
        message_id: MessageId,
    },
    ClearHistory {
        id: Option<String>,
    },
    /// Start a fresh durable session in this process. Requires an idle session.
    NewSession {
        id: Option<String>,
    },
    HistorySummary {
        id: Option<String>,
    },
    UsageSummary {
        id: Option<String>,
    },
    Remember {
        id: Option<String>,
        kind: String,
        content: String,
    },
    Approval {
        id: Option<String>,
        approval_id: String,
        approved: bool,
        note: Option<String>,
        cache: ApprovalCacheScope,
    },
    UserInput {
        id: Option<String>,
        request_id: String,
        response: UserInputResponse,
    },
    Cancel {
        id: Option<String>,
        target_id: String,
    },
    SetPermissionMode {
        id: Option<String>,
        mode: PermissionMode,
    },
    SetModel {
        id: Option<String>,
        model: String,
    },
    /// Exact session-only routing for a fresh delegated task. Unlike user
    /// defaults, this never updates a role's shared last-model preference.
    SetModelConfiguration {
        id: Option<String>,
        model: ModelRef,
        reasoning: ReasoningConfig,
        stream: bool,
    },
    SetReasoningEffort {
        id: Option<String>,
        effort: Option<String>,
    },
    SetReasoningEnabled {
        id: Option<String>,
        enabled: bool,
    },
    ConfigSummary {
        id: Option<String>,
    },
    ReloadTools {
        id: Option<String>,
    },
    Shutdown {
        id: Option<String>,
    },
}

impl StdioRequest {
    pub fn id(&self) -> Option<String> {
        match self {
            Self::CommandCatalog { id } | Self::ExecuteCommand { id, .. } => id.clone(),
            Self::Send { id, .. }
            | Self::EditQueuedMessage { id, .. }
            | Self::DeleteQueuedMessage { id, .. }
            | Self::ClearHistory { id }
            | Self::NewSession { id }
            | Self::HistorySummary { id }
            | Self::UsageSummary { id }
            | Self::Remember { id, .. }
            | Self::Approval { id, .. }
            | Self::UserInput { id, .. }
            | Self::Cancel { id, .. }
            | Self::SetPermissionMode { id, .. }
            | Self::SetModel { id, .. }
            | Self::SetModelConfiguration { id, .. }
            | Self::SetReasoningEffort { id, .. }
            | Self::SetReasoningEnabled { id, .. }
            | Self::ConfigSummary { id }
            | Self::ReloadTools { id }
            | Self::Shutdown { id } => id.clone(),
        }
    }
}
