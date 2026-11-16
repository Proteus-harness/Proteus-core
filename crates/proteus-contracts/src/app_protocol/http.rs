use std::{collections::BTreeMap, path::PathBuf};

use crate::contracts::{ApprovalCacheScope, UserInputResponse};
use serde::Deserialize;

use crate::domain::PermissionMode;

#[derive(Debug, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendRequest {
    pub id: Option<String>,
    pub text: String,
    #[serde(default)]
    pub images: Vec<crate::domain::ImageAttachment>,
    #[serde(default)]
    pub options: crate::domain::RunOptions,
    pub session_dir: PathBuf,
}

#[derive(Debug, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRequest {
    pub id: Option<String>,
    pub approval_id: String,
    pub approved: bool,
    pub note: Option<String>,
    #[serde(default)]
    pub cache: ApprovalCacheScope,
}

#[derive(Debug, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserInputRequest {
    pub id: Option<String>,
    pub request_id: String,
    pub response: UserInputResponse,
}

#[derive(Debug, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelRequest {
    pub id: Option<String>,
    pub target_id: String,
}

#[derive(Debug, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetPermissionModeRequest {
    pub id: Option<String>,
    pub mode: PermissionMode,
    pub session_dir: PathBuf,
}

#[derive(Debug, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetModelRequest {
    pub id: Option<String>,
    pub model: String,
    pub session_dir: PathBuf,
}

#[derive(Debug, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetSessionSettingRequest {
    pub id: Option<String>,
    pub session_dir: PathBuf,
    pub setting: super::settings::SessionSettingUpdate,
}

#[derive(Debug, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetReasoningEffortRequest {
    pub id: Option<String>,
    pub effort: Option<String>,
    pub session_dir: PathBuf,
}

#[derive(Debug, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetReasoningEnabledRequest {
    pub id: Option<String>,
    pub enabled: bool,
    pub session_dir: PathBuf,
}

#[derive(Debug, Clone, Default, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetConfigBuilderRequest {
    #[serde(default)]
    pub addon_settings: Option<super::addons::AppAddonsUpdate>,
    /// None preserves the ordered hook selection; Some replaces it.
    #[serde(default)]
    pub hooks: Option<Vec<String>>,
    #[serde(default)]
    pub modules: BTreeMap<String, String>,
    #[serde(default)]
    pub module_config: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
    /// `None` — не трогать `tools.enabled`; `Some` — заменить список целиком.
    #[serde(default)]
    pub tools_enabled: Option<Vec<String>>,
    /// Absent preserves the source choice; null removes it; a string selects a preset.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_model_choice"
    )]
    pub model: Option<Option<String>>,
    /// None preserves named definitions; Some replaces the owned catalogue.
    #[serde(default)]
    pub models: Option<BTreeMap<String, super::config_builder::ConfigBuilderModelPreset>>,
    /// Optional compare-and-swap baseline for a catalogue replacement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_models: Option<BTreeMap<String, super::config_builder::ConfigBuilderModelPreset>>,
    /// Partial direct-role selections; a null value restores parent inheritance.
    #[serde(default)]
    pub role_models: Option<BTreeMap<String, Option<String>>>,
    /// `None` — не трогать `[permissions] mode`.
    #[serde(default)]
    pub permission_mode: Option<PermissionMode>,
    /// Register installed process packages without selecting their exports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub component_packages: Vec<ComponentPackageInstall>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentPackageInstall {
    pub id: String,
    /// Absolute executable followed by its arguments; never a shell command.
    pub command: Vec<String>,
    /// Explicit environment names shared with this new component only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_allowlist: Vec<String>,
    /// Additive settings for this new package's concrete exports only.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub initial_config: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
    /// Declared package exports, in the same shape as module.json.
    pub exports: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub replace: Vec<ComponentReplacement>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentReplacement {
    pub id: String,
    /// Expected current executable; a stale snapshot cannot replace a new command.
    pub command: String,
    /// Physical executable selected by the client, required for PATH commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_executable: Option<PathBuf>,
}

#[derive(Debug, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeSessionRequest {
    pub id: Option<String>,
    pub session_dir: PathBuf,
}

#[derive(Debug, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewSessionRequest {
    pub id: Option<String>,
    pub source_session_dir: Option<PathBuf>,
    /// Folder the new session works in; an existing directory.
    #[serde(default)]
    pub workspace: Option<PathBuf>,
}

/// Lifecycle acknowledgement; model metadata is read separately through `/config`.
#[derive(Debug, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionOpened {
    pub session_dir: PathBuf,
    /// Exact live workspace, available before any catalog or model discovery.
    pub workspace_path: PathBuf,
    pub activity: super::AppSessionActivity,
}

#[derive(Debug, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteSessionRequest {
    pub id: Option<String>,
    pub session_dir: PathBuf,
}

use crate::domain::MessageId;
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditQueuedMessageRequest {
    pub id: Option<String>,
    pub session_dir: PathBuf,
    pub message_id: MessageId,
    pub text: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteQueuedMessageRequest {
    pub id: Option<String>,
    pub session_dir: PathBuf,
    pub message_id: MessageId,
}

fn deserialize_model_choice<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod model_choice_tests {
    use super::*;
    #[test]
    fn model_choice_distinguishes_preserve_clear_and_select() {
        for (wire, expected) in [
            (serde_json::json!({}), None),
            (serde_json::json!({"model":null}), Some(None)),
            (
                serde_json::json!({"model":"cheap"}),
                Some(Some("cheap".into())),
            ),
        ] {
            let value: SetConfigBuilderRequest = serde_json::from_value(wire).unwrap();
            assert_eq!(value.model, expected);
            let round_trip: SetConfigBuilderRequest =
                serde_json::from_value(serde_json::to_value(&value).unwrap()).unwrap();
            assert_eq!(round_trip.model, expected);
            assert_eq!(
                serde_json::to_value(value).unwrap().get("model").is_some(),
                expected.is_some()
            );
        }
    }
}
