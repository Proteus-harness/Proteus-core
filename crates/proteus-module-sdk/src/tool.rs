//! Typed adapter for the existing tool/v6 contract; it defines no new slot.

use anyhow::{Result, bail};
use proteus_contracts::{
    contracts::{
        PROCESS_TOOL_CONTRACT_VERSION, PROCESS_TOOL_INVOKE_METHOD, PROCESS_TOOL_LIST_METHOD,
        ProcessComponentExportInitialize, ProcessComponentExportManifest, ProcessToolDefinition,
        ProcessToolInvokeInput, ProcessToolInvokeResponse, ProcessToolListResponse,
    },
    domain::{ToolResult, tool_validation::validate_tool_input_schema, validate_tool_call_args},
    process_module::ToolModuleHost,
};
use serde_json::Value;

use crate::{
    Export,
    hosts::{HostBridge, ToolHostBridge},
};

/// A pack of tools belonging to one configured process export.
///
/// Return ordinary domain failures as `Ok(ToolResult::error(..))`; return `Err`
/// for a failed module operation. Check `host.is_cancelled()` during long work.
/// Cancellation is cooperative; the host owns timeout/termination policy.
pub trait ToolModule: Send + Sync {
    fn definitions(&self) -> Vec<ProcessToolDefinition>;

    fn invoke(
        &self,
        input: ProcessToolInvokeInput,
        host: &mut dyn ToolModuleHost,
    ) -> Result<ToolResult>;
}

pub struct ToolExport<T> {
    binding: ProcessComponentExportInitialize,
    tools: T,
    definitions: Vec<ProcessToolDefinition>,
}

impl<T: ToolModule> ToolExport<T> {
    /// Bind one requested tool/v6 export. The factory must also check module_id
    /// and parse module_config before choosing its implementation.
    pub fn new(binding: ProcessComponentExportInitialize, tools: T) -> Result<Self> {
        if binding.slot != "tool" || binding.contract_version != PROCESS_TOOL_CONTRACT_VERSION {
            bail!("ToolExport requires tool/{PROCESS_TOOL_CONTRACT_VERSION}");
        }
        let definitions = tools.definitions();
        let mut names = std::collections::BTreeSet::new();
        for definition in &definitions {
            validate_tool_input_schema(&definition.spec).map_err(anyhow::Error::msg)?;
            if !names.insert(&definition.spec.name) {
                bail!("tool definition name is repeated: {}", definition.spec.name);
            }
        }
        Ok(Self {
            binding,
            tools,
            definitions,
        })
    }
}

impl<T: ToolModule> Export for ToolExport<T> {
    fn manifest(&self) -> ProcessComponentExportManifest {
        ProcessComponentExportManifest {
            slot: self.binding.slot.clone(),
            module_id: self.binding.module_id.clone(),
            contract_version: self.binding.contract_version.clone(),
            composition: self.binding.composition,
            module_features: Vec::new(),
            config_schema: None,
        }
    }

    fn invoke(&self, method: &str, params: Value, host: &HostBridge) -> Result<Value> {
        match method {
            PROCESS_TOOL_LIST_METHOD => {
                if !params.is_null() {
                    bail!("tool list params must be null");
                }
                Ok(serde_json::to_value(ProcessToolListResponse::new(
                    self.definitions.clone(),
                ))?)
            }
            PROCESS_TOOL_INVOKE_METHOD => {
                let mut input: ProcessToolInvokeInput = serde_json::from_value(params)?;
                // Raw provider arguments are canonical when present. Validate
                // and execute the same value, matching Core's normalization.
                if let Some(raw) = input.call.raw_arguments.as_deref() {
                    input.call.args = serde_json::from_str(raw)?;
                }
                let definition = self
                    .definitions
                    .iter()
                    .find(|definition| definition.spec.name == input.call.name)
                    .ok_or_else(|| anyhow::anyhow!("unknown tool {}", input.call.name))?;
                if let Some(error) = validate_tool_call_args(&input.call, &definition.spec) {
                    bail!("{error}");
                }
                let call_id = input.call.id.clone();
                let result = self
                    .tools
                    .invoke(input, &mut ToolHostBridge(host.clone()))?;
                if result.call_id != call_id {
                    bail!("tool result call_id does not match the invocation");
                }
                Ok(serde_json::to_value(ProcessToolInvokeResponse::new(
                    result,
                ))?)
            }
            _ => bail!("unsupported tool method {method:?}"),
        }
    }
}
