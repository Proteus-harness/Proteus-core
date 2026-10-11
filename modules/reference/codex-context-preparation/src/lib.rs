//! Pinned Codex automatic compaction as an ordinary preparation hook.
mod budget;
mod compaction;
mod config;
mod config_schema;
pub use config_schema::config_schema;
mod history;
mod recovery;
mod summary;

use proteus_contracts::{
    contracts::{HookEvent, HookInput, HookResponse},
    process_module::{HookModule, HookModuleHost, ModuleRegistry, ProcessModuleError},
};

pub(crate) const MODULE_ID: &str = "codex_compact";

pub struct CodexPreparationHook(config::CompactorConfig);

impl HookModule for CodexPreparationHook {
    fn invoke_json(
        &self,
        input_json: String,
        host: &mut dyn HookModuleHost,
    ) -> Result<String, ProcessModuleError> {
        let hook: HookInput = serde_json::from_str(&input_json).map_err(error)?;
        let response = match hook.event {
            HookEvent::BeforeContext {
                mut input, output, ..
            } => {
                // Every participant transforms the validated preceding candidate.
                input.messages = output.messages.clone();
                if output.changed {
                    input.token_estimate = output.token_estimate;
                }
                let prepared = compaction::compact(input, &self.0, host)?;
                if output.changed && !prepared.changed {
                    HookResponse::Continue
                } else {
                    HookResponse::ContextPreparation { output: prepared }
                }
            }
            _ => HookResponse::Continue,
        };
        serde_json::to_string(&response).map_err(error)
    }
}

fn error(error: impl std::fmt::Display) -> ProcessModuleError {
    ProcessModuleError::new(error.to_string())
}

pub fn register_modules(registry: &mut dyn ModuleRegistry) -> Result<(), ProcessModuleError> {
    let config = config::CompactorConfig::parse(registry.module_config()).map_err(error)?;
    registry.register_hook(MODULE_ID.to_owned(), Box::new(CodexPreparationHook(config)))
}

#[cfg(test)]
mod tests;
