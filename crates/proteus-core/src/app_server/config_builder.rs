mod behaviors;
mod component_packages;
mod inline_source;
mod model_connections;
mod models;
mod persistence;
mod plugins;
#[cfg(test)]
mod tests;
mod transaction;
pub(super) use persistence::config_builder_target_path;
#[cfg(test)]
use persistence::persist_config_builder;
use persistence::{persist_config_builder_with_components, validate_module_config_toml};
pub(super) use transaction::path_lock;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use serde_json::Value;

use crate::{
    core::{
        AppConfig, ModuleCatalogEntrySummary, ModuleSourceTopology, ModuleTopology, ModulesConfig,
        TopologyBuildInput, TopologySnapshot, build_topology_snapshot,
        core_slots::{CoreSlotSelection, core_slot_descriptor_by_id},
    },
    domain::PermissionMode,
};

use super::{AppServerHandle, prepare_assembly};

pub use proteus_contracts::app_protocol::config_builder::{
    ConfigBuilderModelPreset, ConfigBuilderModelRole, ConfigBuilderModule,
    ConfigBuilderModuleSelection, ConfigBuilderSlot, ConfigBuilderSnapshot, ConfigBuilderTool,
    ConfigBuilderWarning,
};

impl AppServerHandle {
    pub async fn config_builder_snapshot(&self) -> ConfigBuilderSnapshot {
        let (runtime, _, _, mode) = self.runtime.configuration_view().await;
        let tools = &runtime.registry.tool_inventory;
        let topology = build_topology_snapshot(TopologyBuildInput {
            plan: &runtime.assembly_plan,
            tools,
            module_epoch: runtime.epoch,
            permission_mode: mode,
            extra_warnings: Vec::new(),
        });
        let baseline = self.config.read().await;
        let mut snapshot = config_builder_snapshot_from_topology(
            &topology,
            &baseline,
            &runtime.registry.process_tool_specs,
        );
        drop(baseline);
        if let Some(message) = self.profile_error.lock().await.clone() {
            snapshot.warnings.push(ConfigBuilderWarning {
                severity: "error".into(),
                message,
            });
        }
        let descriptions = runtime.registry.config_schemas().await;
        for module in snapshot
            .slots
            .iter_mut()
            .flat_map(|slot| &mut slot.modules)
            .chain(snapshot.hook_modules.iter_mut())
            .chain(snapshot.model_modules.iter_mut())
        {
            module.config_schema = descriptions
                .schemas
                .get(&(module.slot.clone(), module.id.clone()))
                .cloned();
        }
        for export in snapshot
            .plugins
            .iter_mut()
            .flat_map(|plugin| &mut plugin.exports)
        {
            export.config_schema = descriptions
                .schemas
                .get(&(export.slot.clone(), export.id.clone()))
                .cloned();
        }
        snapshot
            .warnings
            .extend(
                descriptions
                    .errors
                    .into_iter()
                    .map(|message| ConfigBuilderWarning {
                        severity: "warning".into(),
                        message,
                    }),
            );
        snapshot
    }

    pub async fn set_config_builder(
        &self,
        modules: BTreeMap<String, String>,
        hooks: Option<Vec<String>>,
        module_config: BTreeMap<String, BTreeMap<String, Value>>,
        tools_enabled: Option<Vec<String>>,
        active_provider: Option<String>,
        permission_mode: Option<PermissionMode>,
    ) -> Result<ConfigBuilderSnapshot> {
        self.set_profile_config(
            proteus_contracts::app_protocol::http::SetConfigBuilderRequest {
                modules,
                hooks,
                module_config,
                tools_enabled,
                model: active_provider.map(Some),
                models: None,
                expected_models: None,
                role_models: None,
                modes: None,
                expected_modes: None,
                permission_mode,
                addon_settings: None,
                component_packages: Vec::new(),
                remove_model_connections: Vec::new(),
            },
        )
        .await
    }

    pub async fn set_profile_config(
        &self,
        request: proteus_contracts::app_protocol::http::SetConfigBuilderRequest,
    ) -> Result<ConfigBuilderSnapshot> {
        let this = self.clone();
        tokio::spawn(async move { this.set_config_builder_owned(request).await })
            .await
            .context("config save task failed")?
    }

    async fn set_config_builder_owned(
        &self,
        request: proteus_contracts::app_protocol::http::SetConfigBuilderRequest,
    ) -> Result<ConfigBuilderSnapshot> {
        let proteus_contracts::app_protocol::http::SetConfigBuilderRequest {
            modules,
            hooks,
            module_config,
            tools_enabled,
            model,
            models,
            expected_models,
            role_models,
            modes,
            expected_modes,
            permission_mode,
            addon_settings,
            component_packages,
            remove_model_connections,
        } = request;
        let config_path = self
            .config_path
            .as_deref()
            .ok_or_else(|| anyhow!("config path is not available; cannot persist config"))?;
        let path_lock = transaction::path_lock(config_path)?;
        let _path_guard = path_lock.lock_owned().await;
        let current_plan = self.runtime.assembly_plan().await;
        validate_config_builder_modules(&modules, current_plan.catalog_entries())?;

        let mut next_config =
            if tokio::fs::try_exists(crate::core::physical_config_path(config_path)).await? {
                AppConfig::load(Some(config_path)).await?
            } else {
                self.config.read().await.clone()
            };
        if let Some(expected) = expected_models {
            anyhow::ensure!(
                models.is_some(),
                "expected_models requires a model catalogue replacement"
            );
            anyhow::ensure!(
                expected == models::presets(&next_config),
                "model catalogue changed; refresh settings before saving"
            );
        }
        let model_edits = models::proposal(
            &next_config,
            model.as_ref(),
            models.as_ref(),
            role_models.as_ref(),
        )?;
        let mode_edits =
            behaviors::proposal(&next_config, modes.as_ref(), expected_modes.as_ref())?;
        if model_edits.is_some() || mode_edits.is_some() {
            if model_edits
                .as_ref()
                .is_some_and(|edits| matches!(edits.selected, Some(None)))
            {
                anyhow::ensure!(
                    AppConfig::inherited_model_choice_key(config_path)?.is_none(),
                    "cannot remove an inherited model selection from an additive include wrapper"
                );
            }
            next_config =
                AppConfig::load_with_settings_edits(config_path, model_edits.as_ref(), mode_edits)
                    .await?;
        }
        let mut component_ids =
            component_packages::apply(&mut next_config, component_packages, &self.cwd)?;
        if let Some(update) = addon_settings {
            if update
                .addons
                .disabled_skills
                .iter()
                .any(|id| id.trim().is_empty())
            {
                anyhow::bail!("disabled skill id must not be empty");
            }
            let resolved = crate::core::agent_plugins::resolve(&update.addons, &self.cwd);
            if let Some(plugin) = resolved
                .plugins
                .iter()
                .find(|plugin| plugin.error.is_some())
            {
                anyhow::bail!(
                    "invalid Agent Plugin {}: {}",
                    plugin.path.display(),
                    plugin.error.as_ref().unwrap()
                );
            }
            next_config.addons = update.addons;
            next_config.tools.mcp_servers = update.mcp_servers;
        }
        if let Some(hooks) = hooks {
            next_config.modules.hooks = hooks;
        }
        for (slot, module_id) in modules {
            set_module_slot(&mut next_config.modules, &slot, module_id)?;
        }
        for (slot, values) in module_config {
            next_config.module_config.insert(slot, values);
        }
        if let Some(tools_enabled) = tools_enabled {
            next_config.tools.enabled = tools_enabled;
        }
        if let Some(mode) = permission_mode {
            next_config.permissions.mode = mode;
        }
        component_ids.extend(model_connections::remove(
            &mut next_config,
            config_path,
            &remove_model_connections,
        )?);
        next_config.validate_semantics()?;
        let target = config_builder_target_path(Some(config_path))
            .context("config target is not available")?;
        if !crate::core::physical_config_path(&target)
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            validate_module_config_toml(&next_config.module_config)?;
        }

        let assembly =
            prepare_assembly(&next_config, &self.cwd, self.config_path.as_deref()).await?;
        self.publish_profile(next_config.clone(), assembly, permission_mode, || async {
            persist_config_builder_with_components(&target, &next_config, &component_ids).await
        })
        .await?;

        Ok(self.config_builder_snapshot().await)
    }
}

pub(super) fn config_builder_snapshot_from_topology(
    topology: &TopologySnapshot,
    config: &AppConfig,
    process_tool_specs: &[(crate::contracts::ToolSource, crate::domain::ToolSpec)],
) -> ConfigBuilderSnapshot {
    let target_path = config_builder_target_path(topology.config_path.as_deref().map(Path::new));
    let modules = topology.modules.clone();
    let slots = topology
        .slots
        .iter()
        .filter(|slot| is_config_builder_module_slot(&slot.id))
        .map(|slot| ConfigBuilderSlot {
            id: slot.id.clone(),
            title: slot.title.clone(),
            responsibility: slot.responsibility.clone(),
            active_module: slot.active_module.clone(),
            required: slot.required,
            category: slot.category.clone(),
            order: slot.order,
            modules: modules
                .iter()
                .filter(|module| module.slot == slot.id)
                .map(config_builder_module)
                .collect(),
        })
        .collect();

    let (tools, tool_warnings) = plugins::builder_tools(topology, process_tool_specs, config);
    let plugins = plugins::builder_plugins(config, &tools);
    ConfigBuilderSnapshot {
        addon_settings: proteus_contracts::app_protocol::addons::AppAddonsUpdate {
            addons: config.addons.clone(),
            mcp_servers: config.tools.mcp_servers.clone(),
        },
        config_path: topology.config_path.clone(),
        writable: target_path.is_some(),
        target_path: target_path.map(|path| path.display().to_string()),
        model: models::source_choice(config),
        model_writable: models::choice_writable(config),
        models: models::presets(config),
        models_writable: config.model_source.catalogue_writable,
        modes: config.modes.clone(),
        modes_writable: config.behavior_source.writable,
        model_roles: models::roles(config),
        model_modules: modules
            .iter()
            .filter(|module| module.slot == "model")
            .map(|module| {
                let mut item = config_builder_module(module);
                item.active = config
                    .active_provider
                    .as_ref()
                    .and_then(|alias| config.providers.get(alias))
                    .is_some_and(|preset| preset.provider == module.id);
                item
            })
            .collect(),
        permission_mode: permission_mode_str(config.permissions.mode),
        permission_modes: PermissionMode::SELECTABLE
            .iter()
            .map(|mode| mode.as_str().to_owned())
            .collect(),
        active_modules: config
            .modules
            .iter()
            .map(|(kind, id)| ConfigBuilderModuleSelection {
                slot: kind.as_str().to_owned(),
                id: id.to_owned(),
            })
            .collect(),
        hooks: config.modules.hooks.clone(),
        hook_modules: modules
            .iter()
            .filter(|module| module.slot == "hook")
            .map(|module| {
                let mut item = config_builder_module(module);
                item.active = config.modules.hooks.contains(&module.id);
                item
            })
            .collect(),
        module_config: config.module_config.clone(),
        tools_enabled: config.tools.enabled.clone(),
        tools,
        plugins,
        warnings: topology
            .warnings
            .iter()
            .map(|warning| ConfigBuilderWarning {
                severity: warning.severity.clone(),
                message: warning.message.clone(),
            })
            .chain(tool_warnings)
            .collect(),
        slots,
    }
}

fn config_builder_module(module: &ModuleTopology) -> ConfigBuilderModule {
    ConfigBuilderModule {
        config_schema: None,
        id: module.id.clone(),
        slot: module.slot.clone(),
        active: module.active,
        source: module_source_label(&module.source),
        version: module.version.clone(),
        api_version: module.api_version.clone(),
        capabilities: module.capabilities.clone(),
        description: module.description.clone(),
    }
}

fn module_source_label(source: &ModuleSourceTopology) -> String {
    match source {
        ModuleSourceTopology::Builtin => "builtin".to_owned(),
        ModuleSourceTopology::Process => "process".to_owned(),
        ModuleSourceTopology::Config => "config".to_owned(),
        ModuleSourceTopology::Unknown => "unknown".to_owned(),
    }
}

fn is_config_builder_module_slot(slot: &str) -> bool {
    core_slot_descriptor_by_id(slot)
        .is_some_and(|descriptor| descriptor.selection == CoreSlotSelection::ModulesConfig)
}

/// Каноническое имя для `POST /mode` и `[permissions] mode`.
fn permission_mode_str(mode: PermissionMode) -> String {
    mode.as_str().to_owned()
}

pub(super) fn validate_config_builder_modules(
    modules: &BTreeMap<String, String>,
    catalog_entries: &[ModuleCatalogEntrySummary],
) -> Result<()> {
    let known = catalog_entries
        .iter()
        .map(|entry| (entry.slot.as_str(), entry.id.as_str()))
        .collect::<BTreeSet<_>>();
    for (slot, module_id) in modules {
        if !is_config_builder_module_slot(slot) {
            anyhow::bail!("unsupported config builder slot: {slot}");
        }
        if !known.contains(&(slot.as_str(), module_id.as_str())) {
            anyhow::bail!("module is not registered for slot {slot}: {module_id}");
        }
    }
    Ok(())
}

pub(super) fn set_module_slot(
    modules: &mut ModulesConfig,
    slot: &str,
    module_id: String,
) -> Result<()> {
    if !modules.set_by_slot_id(slot, module_id) {
        anyhow::bail!("unsupported config builder slot: {slot}");
    }
    Ok(())
}

#[cfg(test)]
mod hook_tests {
    use super::*;

    #[tokio::test]
    async fn builder_save_preserves_hook_order_when_editing_other_selections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = crate::test_model::config();
        config.modules.hooks = vec!["second".into(), "first".into()];
        config.modules.context = Some("simple".into());
        persist_config_builder(&path, &config).await.unwrap();
        config.modules.context = Some("repo_aware".into());
        persist_config_builder(&path, &config).await.unwrap();
        let saved: toml::Value =
            toml::from_str(&tokio::fs::read_to_string(&path).await.unwrap()).unwrap();
        assert_eq!(
            saved["modules"]["hooks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_str().unwrap())
                .collect::<Vec<_>>(),
            ["second", "first"]
        );
        assert_eq!(saved["modules"]["context"].as_str(), Some("repo_aware"));
        config.modules.hooks.clear();
        persist_config_builder(&path, &config).await.unwrap();
        let saved: toml::Value =
            toml::from_str(&tokio::fs::read_to_string(&path).await.unwrap()).unwrap();
        assert!(saved["modules"]["hooks"].as_array().unwrap().is_empty());
    }
}
