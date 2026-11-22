use serde_json::Value;
use std::collections::BTreeMap;

mod plugins;
pub use plugins::{ConfigBuilderPlugin, ConfigBuilderPluginExport, ConfigBuilderToolPack};

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigBuilderSnapshot {
    pub addon_settings: super::addons::AppAddonsUpdate,
    pub config_path: Option<String>,
    pub target_path: Option<String>,
    pub writable: bool,
    pub model: Option<String>,
    pub model_writable: bool,
    pub models: BTreeMap<String, ConfigBuilderModelPreset>,
    /// Inherited catalogues must be edited at their owning source.
    pub models_writable: bool,
    pub model_roles: Vec<ConfigBuilderModelRole>,
    pub model_modules: Vec<ConfigBuilderModule>,
    pub modes: BTreeMap<String, crate::domain::BehaviorModeDefinition>,
    pub modes_writable: bool,
    /// Persisted `[permissions] mode` (snake_case) — то, что редактирует
    /// builder. Runtime mode может отличаться после `POST /mode`.
    pub permission_mode: String,
    pub permission_modes: Vec<String>,
    pub active_modules: Vec<ConfigBuilderModuleSelection>,
    pub hooks: Vec<String>,
    pub hook_modules: Vec<ConfigBuilderModule>,
    pub module_config: BTreeMap<String, BTreeMap<String, Value>>,
    pub tools_enabled: Vec<String>,
    pub tools: Vec<ConfigBuilderTool>,
    pub plugins: Vec<ConfigBuilderPlugin>,
    pub slots: Vec<ConfigBuilderSlot>,
    pub warnings: Vec<ConfigBuilderWarning>,
}

/// A reusable named model definition, independent of its connection module.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigBuilderModelPreset {
    pub provider: String,
    pub model: String,
    pub stream: bool,
    pub reasoning: crate::domain::ReasoningConfig,
    pub reasoning_efforts: Vec<String>,
}

/// Exact source selection for one direct child role. An omitted selection
/// inherits the parent's current turn model; complex/external sources are read-only.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigBuilderModelRole {
    pub name: String,
    pub model: Option<String>,
    pub editable: bool,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigBuilderModuleSelection {
    pub slot: String,
    pub id: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigBuilderSlot {
    pub id: String,
    pub title: String,
    pub responsibility: String,
    pub active_module: Option<String>,
    pub required: bool,
    pub category: String,
    pub order: u32,
    pub modules: Vec<ConfigBuilderModule>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigBuilderModule {
    pub id: String,
    pub slot: String,
    pub active: bool,
    pub source: String,
    pub version: String,
    pub api_version: String,
    pub capabilities: Vec<String>,
    pub description: Option<String>,
    pub config_schema: Option<crate::domain::ModuleConfigSchema>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigBuilderWarning {
    pub severity: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigBuilderTool {
    pub name: String,
    pub source: String,
    pub owner: Option<crate::contracts::ProcessToolOwner>,
    pub safety: String,
    pub description: String,
    pub enabled: bool,
    /// `tools_enabled` cannot toggle this tool; preserve any existing entry on save.
    pub runtime_managed: bool,
    pub registered: bool,
}
