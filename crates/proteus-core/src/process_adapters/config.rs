use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use proteus_module_protocol::{
    ProcessComponentBinding, ProcessExportBinding,
    v3::{
        COMPONENT_MAX_FRAME_BYTES_ENV, ComponentBroker, ComponentBrokerOptions,
        ComponentTransportConfig,
    },
};
use proteus_process_host::ProcessSpec;
use serde::{Deserialize, Serialize};

use crate::contracts::ProcessComponentExportRef;
use proteus_contracts::domain::ProcessSandboxConfig;

use super::sandbox::{resolve_process_cwd, resolve_sandbox};

const DEFAULT_HANDSHAKE_TIMEOUT_MS: u64 = 30_000;

/// Host-owned launch description for one persistent process component.
///
/// Export identities are nested as `exports.<slot>.<module_id>` so config
/// includes merge them as objects instead of replacing one descriptor array.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessComponentConfig {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    env_allowlist: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "ProcessSandboxConfig::is_trusted")]
    sandbox: ProcessSandboxConfig,
    #[serde(default)]
    handshake_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transport: Option<ComponentTransportConfig>,
    #[serde(default)]
    description: Option<String>,
    /// Opaque presentation reference; this process host never opens or fetches it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    icon: Option<String>,
    #[serde(default)]
    exports: BTreeMap<String, BTreeMap<String, ProcessExportLaunchConfig>>,
}

/// Per-export launch controls. Contract identity comes from the containing map
/// keys; opaque implementation config remains in `module_config.<slot>.<id>`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessExportLaunchConfig {
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    description: Option<String>,
}

impl ProcessExportLaunchConfig {
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

impl ProcessComponentConfig {
    pub fn process_spec(&self, workspace: &Path) -> Result<ProcessSpec> {
        self.validate_transport("component.transport")?;
        let workspace = std::fs::canonicalize(workspace).with_context(|| {
            format!(
                "failed to resolve component workspace {}",
                workspace.display()
            )
        })?;
        let mut spec = ProcessSpec::new(self.command.clone())
            .args(self.args.clone())
            .env_allowlist(self.env_allowlist.clone())
            .envs(self.env.clone())
            .env(
                COMPONENT_MAX_FRAME_BYTES_ENV,
                self.transport
                    .unwrap_or_default()
                    .max_frame_bytes
                    .to_string(),
            )
            .cwd(resolve_process_cwd(self.cwd.as_deref(), &workspace)?);
        self.validate_sandbox("component.sandbox")?;
        if let Some(sandbox) = resolve_sandbox(&self.sandbox, &workspace)? {
            spec = spec.sandbox(sandbox);
            spec.validate_launch()?;
        }
        Ok(spec)
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    /// Replace only the executable, retaining host-owned launch/export controls.
    pub(crate) fn set_command(&mut self, command: String) {
        self.command = command;
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub(crate) fn set_icon(&mut self, icon: String) {
        self.icon = Some(icon);
    }

    pub fn icon(&self) -> Option<&str> {
        self.icon.as_deref()
    }

    /// Same serialization as the launch descriptor before presentation metadata;
    /// icons never invalidate saved/scoped model selections.
    pub(crate) fn execution_identity(&self) -> impl Serialize + '_ {
        #[derive(Serialize)]
        struct Identity<'a> {
            command: &'a str,
            args: &'a [String],
            cwd: &'a Option<PathBuf>,
            env_allowlist: &'a [String],
            env: &'a BTreeMap<String, String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            sandbox: Option<&'a ProcessSandboxConfig>,
            handshake_timeout_ms: &'a Option<u64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            transport: &'a Option<ComponentTransportConfig>,
            description: &'a Option<String>,
            exports: &'a BTreeMap<String, BTreeMap<String, ProcessExportLaunchConfig>>,
        }
        Identity {
            command: &self.command,
            args: &self.args,
            cwd: &self.cwd,
            env_allowlist: &self.env_allowlist,
            env: &self.env,
            sandbox: (!self.sandbox.is_trusted()).then_some(&self.sandbox),
            handshake_timeout_ms: &self.handshake_timeout_ms,
            transport: &self.transport,
            description: &self.description,
            exports: &self.exports,
        }
    }

    pub fn exports(&self) -> impl Iterator<Item = (&str, &str, &ProcessExportLaunchConfig)> {
        self.exports.iter().flat_map(|(slot, modules)| {
            modules
                .iter()
                .map(move |(module_id, launch)| (slot.as_str(), module_id.as_str(), launch))
        })
    }

    pub(crate) fn remove_export(&mut self, slot: &str, id: &str) {
        if let Some(exports) = self.exports.get_mut(slot) {
            exports.remove(id);
            if exports.is_empty() {
                self.exports.remove(slot);
            }
        }
    }

    pub fn validate_for(&self, component_id: &str, path: &str) -> Result<()> {
        if component_id.trim().is_empty() {
            bail!("{path} component id must not be empty");
        }
        if self.command.trim().is_empty() {
            bail!("{path}.command must not be empty");
        }
        if self.handshake_timeout_ms == Some(0) {
            bail!("{path}.handshake_timeout_ms must be greater than zero");
        }
        if self.exports.is_empty() {
            bail!("{path}.exports must not be empty");
        }
        for (slot, modules) in &self.exports {
            if slot.trim().is_empty() {
                bail!("{path}.exports contains an empty slot");
            }
            if modules.is_empty() {
                bail!("{path}.exports.{slot} must not be empty");
            }
            for (module_id, export) in modules {
                if module_id.trim().is_empty() {
                    bail!("{path}.exports.{slot} contains an empty module id");
                }
                if export.timeout_ms == Some(0) {
                    bail!("{path}.exports.{slot}.{module_id}.timeout_ms must be greater than zero");
                }
            }
        }
        self.validate_sandbox(&format!("{path}.sandbox"))?;
        self.validate_transport(&format!("{path}.transport"))?;
        Ok(())
    }

    fn validate_transport(&self, path: &str) -> Result<()> {
        if self.env.contains_key(COMPONENT_MAX_FRAME_BYTES_ENV) {
            bail!(
                "{path}: {COMPONENT_MAX_FRAME_BYTES_ENV} is host-owned; configure transport.max_frame_bytes instead"
            );
        }
        if let Some(transport) = self.transport {
            transport.validate().with_context(|| path.to_owned())?;
        }
        Ok(())
    }

    fn validate_sandbox(&self, path: &str) -> Result<()> {
        if !self.sandbox.is_trusted() && self.exports().count() != 1 {
            bail!(
                "{path}: restricted components require exactly one export; split exports into separate components"
            );
        }
        self.sandbox.validate(path)
    }

    fn handshake_timeout(&self) -> Duration {
        Duration::from_millis(
            self.handshake_timeout_ms
                .unwrap_or(DEFAULT_HANDSHAKE_TIMEOUT_MS),
        )
    }

    fn export(&self, slot: &str, module_id: &str) -> Option<&ProcessExportLaunchConfig> {
        self.exports.get(slot)?.get(module_id)
    }
}

/// Runtime launch handle shared by every typed adapter exported from one
/// component. A catalog owns one launcher; adapters built for the same
/// canonical workspace therefore share one persistent session and lifecycle.
pub(crate) struct ProcessComponentLauncher {
    component_id: String,
    config: ProcessComponentConfig,
    binding: ProcessComponentBinding,
    brokers: Mutex<HashMap<PathBuf, Arc<ComponentBroker>>>,
}

impl std::fmt::Debug for ProcessComponentLauncher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProcessComponentLauncher")
            .field("component_id", &self.component_id)
            .field("config", &self.config)
            .field("binding", &self.binding)
            .finish_non_exhaustive()
    }
}

impl ProcessComponentLauncher {
    pub(crate) fn new(
        component_id: impl Into<String>,
        config: ProcessComponentConfig,
        exports: Vec<ProcessExportBinding>,
    ) -> Result<Arc<Self>> {
        let component_id = component_id.into();
        let binding = ProcessComponentBinding::new(component_id.clone(), exports)?;
        Ok(Arc::new(Self {
            component_id,
            config,
            binding,
            brokers: Mutex::new(HashMap::new()),
        }))
    }

    pub(crate) fn export(
        self: &Arc<Self>,
        slot: &str,
        module_id: &str,
    ) -> Result<ProcessExportConfig> {
        let launch = self.config.export(slot, module_id).ok_or_else(|| {
            anyhow::anyhow!(
                "component {:?} has no configured export {slot}/{module_id}",
                self.component_id
            )
        })?;
        let binding = self
            .binding
            .export(&ProcessComponentExportRef::new(slot, module_id))
            .cloned()
            .map_err(|_| {
                anyhow::anyhow!(
                    "component {:?} has no bound export {slot}/{module_id}",
                    self.component_id
                )
            })?;
        Ok(ProcessExportConfig {
            launcher: Arc::clone(self),
            binding,
            timeout_ms: launch.timeout_ms,
            description: launch.description.clone(),
        })
    }

    fn connect(&self, workspace: &Path) -> Result<Arc<ComponentBroker>> {
        let workspace = std::fs::canonicalize(workspace).with_context(|| {
            format!(
                "failed to resolve component {:?} workspace {}",
                self.component_id,
                workspace.display()
            )
        })?;
        let mut brokers = self.brokers.lock().map_err(|_| {
            anyhow::anyhow!("component {:?} broker cache poisoned", self.component_id)
        })?;
        if let Some(broker) = brokers.get(&workspace) {
            return Ok(Arc::clone(broker));
        }
        let broker = Arc::new(ComponentBroker::connect(
            self.config.process_spec(&workspace)?,
            self.binding.clone(),
            self.config
                .transport
                .unwrap_or_default()
                .apply(ComponentBrokerOptions {
                    handshake_timeout: self.config.handshake_timeout(),
                    ..ComponentBrokerOptions::default()
                })?,
        )?);
        brokers.insert(workspace, Arc::clone(&broker));
        Ok(broker)
    }

    fn release_discovery_brokers(&self) -> Result<()> {
        let released: Vec<_> = {
            let mut brokers = self.brokers.lock().map_err(|_| {
                anyhow::anyhow!("component {:?} broker cache poisoned", self.component_id)
            })?;
            // The cache is the only source of new broker owners. A sole
            // cached owner means no runtime adapter retained this worker.
            brokers
                .extract_if(|_, broker| Arc::strong_count(broker) == 1)
                .collect()
        };
        // Worker shutdown can wait for the broker thread. Drop outside the lock.
        drop(released);
        Ok(())
    }
}

/// One typed module export plus a shared component launcher.
#[derive(Debug, Clone)]
pub(crate) struct ProcessExportConfig {
    launcher: Arc<ProcessComponentLauncher>,
    binding: ProcessExportBinding,
    timeout_ms: Option<u64>,
    description: Option<String>,
}

impl ProcessExportConfig {
    pub(crate) fn component_id(&self) -> &str {
        &self.launcher.component_id
    }

    pub(crate) fn module_id(&self) -> &str {
        &self.binding.module_id
    }

    pub(crate) fn slot(&self) -> &str {
        &self.binding.slot
    }

    pub(crate) fn description(&self) -> Option<&str> {
        self.description
            .as_deref()
            .or_else(|| self.launcher.config.description())
    }

    pub(crate) fn timeout(&self, default_timeout_ms: u64) -> Duration {
        Duration::from_millis(self.timeout_ms.unwrap_or(default_timeout_ms))
    }

    pub(crate) fn validate_for(&self, slot: &str, default_timeout_ms: u64) -> Result<()> {
        if self.slot() != slot {
            bail!(
                "component {:?} export {:?} declares slot {:?}, expected {:?}",
                self.component_id(),
                self.module_id(),
                self.slot(),
                slot
            );
        }
        if default_timeout_ms == 0 && self.timeout_ms.is_none() {
            bail!(
                "component {:?} export {}/{} requires timeout_ms",
                self.component_id(),
                self.slot(),
                self.module_id()
            );
        }
        Ok(())
    }

    pub(crate) fn binding(&self) -> &ProcessExportBinding {
        &self.binding
    }

    pub(crate) fn connect(&self, workspace: &Path) -> Result<Arc<ComponentBroker>> {
        self.launcher.connect(workspace)
    }

    pub(crate) fn release_discovery_brokers(&self) -> Result<()> {
        self.launcher.release_discovery_brokers()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_config_is_strict_and_exports_are_nested_by_identity() {
        let config: ProcessComponentConfig = serde_json::from_value(serde_json::json!({
            "command": "worker",
            "description": "fixture",
            "exports": {
                "context_provider": {
                    "fixture": {"timeout_ms": 1000}
                }
            }
        }))
        .expect("valid config");
        config
            .validate_for("fixture-component", "components.fixture-component")
            .expect("valid component");
        assert_eq!(
            config
                .exports()
                .map(|(slot, id, _)| (slot, id))
                .collect::<Vec<_>>(),
            [("context_provider", "fixture")]
        );

        serde_json::from_value::<ProcessComponentConfig>(serde_json::json!({
            "command": "worker",
            "legacy_slot": "context_provider",
            "exports": {"context_provider": {"fixture": {}}}
        }))
        .expect_err("unknown launch fields must fail");
    }

    #[test]
    fn component_icon_is_retained_only_as_source_presentation_metadata() {
        let mut source = serde_json::json!({"command":"worker","exports":{"model":{"fixture":{}}}});
        let original: ProcessComponentConfig = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(
            serde_json::to_vec(&original).unwrap(),
            serde_json::to_vec(&original.execution_identity()).unwrap()
        );
        source["icon"] = serde_json::json!("assets/model.svg");
        let decorated: ProcessComponentConfig = serde_json::from_value(source).unwrap();
        assert_eq!(decorated.icon(), Some("assets/model.svg"));
        assert_eq!(
            serde_json::to_value(&decorated).unwrap()["icon"],
            "assets/model.svg"
        );
        assert_eq!(
            serde_json::to_vec(&original.execution_identity()).unwrap(),
            serde_json::to_vec(&decorated.execution_identity()).unwrap()
        );
    }

    #[test]
    fn component_config_rejects_empty_exports_and_zero_timeouts() {
        let empty: ProcessComponentConfig = serde_json::from_value(serde_json::json!({
            "command": "worker",
            "exports": {}
        }))
        .expect("shape");
        empty
            .validate_for("fixture", "components.fixture")
            .expect_err("empty exports must fail");

        let zero: ProcessComponentConfig = serde_json::from_value(serde_json::json!({
            "command": "worker",
            "exports": {"context_provider": {"fixture": {"timeout_ms": 0}}}
        }))
        .expect("shape");
        zero.validate_for("fixture", "components.fixture")
            .expect_err("zero export timeout must fail");
    }

    #[test]
    fn component_transport_budget_is_explicit_host_owned_and_execution_identity() {
        let source = serde_json::json!({"command":"worker", "exports":{"model":{"fixture":{}}}});
        let ordinary: ProcessComponentConfig = serde_json::from_value(source.clone()).unwrap();
        let mut raised = source.clone();
        raised["transport"] = serde_json::json!({"max_frame_bytes":134217728});
        let raised: ProcessComponentConfig = serde_json::from_value(raised).unwrap();
        raised
            .validate_for("fixture", "components.fixture")
            .unwrap();
        let workspace = std::env::current_dir().unwrap();
        assert_eq!(
            ordinary.process_spec(&workspace).unwrap().env[COMPONENT_MAX_FRAME_BYTES_ENV],
            "8388608"
        );
        assert_eq!(
            raised.process_spec(&workspace).unwrap().env[COMPONENT_MAX_FRAME_BYTES_ENV],
            "134217728"
        );
        assert_ne!(
            serde_json::to_value(ordinary.execution_identity()).unwrap(),
            serde_json::to_value(raised.execution_identity()).unwrap()
        );
        for value in [0, 134217729] {
            let mut invalid = source.clone();
            invalid["transport"] = serde_json::json!({"max_frame_bytes":value});
            let invalid: ProcessComponentConfig = serde_json::from_value(invalid).unwrap();
            assert!(
                invalid
                    .validate_for("fixture", "components.fixture")
                    .is_err()
            );
        }
        let mut conflict = source;
        conflict["env"] = serde_json::json!({"PROTEUS_MODULE_MAX_FRAME_BYTES":"134217728"});
        let conflict: ProcessComponentConfig = serde_json::from_value(conflict).unwrap();
        assert!(
            conflict
                .validate_for("fixture", "components.fixture")
                .is_err()
        );
    }
}
