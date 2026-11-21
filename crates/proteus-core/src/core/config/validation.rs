//! Source-independent profile rules shared by loading and edited configs.
use anyhow::{Result, bail};

use crate::core::core_slots::{CoreSlotSelection, core_slot_descriptor_by_id};

use super::AppConfig;

impl AppConfig {
    /// Validate typed profile semantics without rereading sources or resolving
    /// instructions. Loaded file instructions already carry both file and text;
    /// their source-shape checks belong to the loader, not this shared pass.
    pub(crate) fn validate_semantics(&self) -> Result<()> {
        self.selected_model_config()?;
        self.validate_module_config_slots()?;
        self.modules.validate_hooks()?;
        self.validate_commands()?;
        Ok(())
    }

    fn validate_module_config_slots(&self) -> Result<()> {
        for slot in self.module_config.keys() {
            if matches!(
                slot.as_str(),
                "tool" | "context_provider" | "model" | "hook"
            ) {
                continue;
            }
            match core_slot_descriptor_by_id(slot) {
                Some(descriptor) if descriptor.selection == CoreSlotSelection::ModulesConfig => {}
                Some(_) => bail!("module_config is not supported for slot {slot:?}"),
                None => bail!("unknown module_config slot {slot:?}"),
            }
        }
        Ok(())
    }
}
