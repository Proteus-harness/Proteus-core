use proteus_contracts::process_module::ProcessModuleError;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct DynamicSelectionConfig {
    pub max_hot_tools: usize,
    pub always_include: Vec<String>,
}

impl DynamicSelectionConfig {
    pub fn from_value(value: &Value) -> Result<Self, ProcessModuleError> {
        let config: Self = serde_json::from_value(value.clone()).map_err(|error| {
            ProcessModuleError::new(format!("invalid tool_selection.dynamic config: {error}"))
        })?;
        if config.max_hot_tools == 0
            || config
                .always_include
                .iter()
                .any(|name| name.trim().is_empty())
        {
            return Err(ProcessModuleError::new(
                "tool_selection.dynamic requires positive max_hot_tools and nonempty tool names",
            ));
        }
        Ok(config)
    }
}

impl Default for DynamicSelectionConfig {
    fn default() -> Self {
        Self {
            max_hot_tools: super::DEFAULT_MAX_HOT_TOOLS,
            always_include: super::DEFAULT_ALWAYS_INCLUDE
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
        }
    }
}
