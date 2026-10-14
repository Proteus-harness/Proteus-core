use std::collections::{BTreeMap, BTreeSet};

use proteus_contracts::app_protocol::addons::AppAddonsUpdate;
use proteus_contracts::app_protocol::config_builder::{ConfigBuilderSnapshot, ConfigBuilderState};
use proteus_contracts::app_protocol::http::SetConfigBuilderRequest;
use serde_json::{Map, Value};

/// Profile area touched by an edit.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Area {
    Provider,
    Mode,
    Tools,
    Hooks,
    Mcp,
    Skills,
    Packages,
    Slot(String),
}

/// Address of one module config value: `module_config[slot][module][key]`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FieldPath {
    pub slot: String,
    pub module: String,
    pub key: String,
}

impl FieldPath {
    pub fn new(slot: &str, module: &str, key: &str) -> Self {
        Self {
            slot: slot.to_string(),
            module: module.to_string(),
            key: key.to_string(),
        }
    }
}

/// Editable copy of the persisted profile settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub modules: BTreeMap<String, String>,
    pub config: BTreeMap<String, BTreeMap<String, Value>>,
    pub hooks: Vec<String>,
    pub tools: BTreeSet<String>,
    pub provider: Option<String>,
    pub mode: String,
    /// Skills, MCP servers and agent plugin packages.
    pub addons: AppAddonsUpdate,
}

impl Draft {
    pub fn from_snapshot(snapshot: &ConfigBuilderSnapshot) -> Self {
        Self {
            modules: snapshot
                .active_modules
                .iter()
                .map(|selection| (selection.slot.clone(), selection.id.clone()))
                .collect(),
            config: snapshot.module_config.clone(),
            hooks: snapshot.hooks.clone(),
            tools: snapshot.tools_enabled.iter().cloned().collect(),
            provider: snapshot.active_provider.clone(),
            mode: snapshot.permission_mode.clone(),
            addons: super::addons::normalized(snapshot.addon_settings.clone()),
        }
    }

    pub fn value(&self, path: &FieldPath) -> Option<&Value> {
        self.config
            .get(&path.slot)?
            .get(&path.module)?
            .as_object()?
            .get(&path.key)
    }

    pub fn set_value(&mut self, path: &FieldPath, value: Option<Value>) {
        match value {
            Some(value) => {
                let module = self
                    .config
                    .entry(path.slot.clone())
                    .or_default()
                    .entry(path.module.clone())
                    .or_insert_with(|| Value::Object(Map::new()));
                if !module.is_object() {
                    *module = Value::Object(Map::new());
                }
                if let Some(object) = module.as_object_mut() {
                    object.insert(path.key.clone(), value);
                }
            }
            None => {
                if let Some(object) = self
                    .config
                    .get_mut(&path.slot)
                    .and_then(|modules| modules.get_mut(&path.module))
                    .and_then(Value::as_object_mut)
                {
                    object.remove(&path.key);
                }
            }
        }
    }

    pub fn set_module(&mut self, slot: &str, module: &str) {
        self.modules.insert(slot.to_string(), module.to_string());
    }

    pub fn set_hook(&mut self, id: &str, enabled: bool) {
        if enabled {
            if !self.hooks.iter().any(|hook| hook == id) {
                self.hooks.push(id.to_string());
            }
        } else {
            self.hooks.retain(|hook| hook != id);
        }
    }

    pub fn set_tool(&mut self, name: &str, enabled: bool) {
        if enabled {
            self.tools.insert(name.to_string());
        } else {
            self.tools.remove(name);
        }
    }

    /// Whole parameter object of one module (`module_config[slot][module]`).
    pub fn module_config(&self, slot: &str, module: &str) -> Option<&Value> {
        self.config.get(slot)?.get(module)
    }

    pub fn set_module_config(&mut self, slot: &str, module: &str, value: Map<String, Value>) {
        self.config
            .entry(slot.to_string())
            .or_default()
            .insert(module.to_string(), Value::Object(value));
    }

    /// Moves an enabled hook up (`offset < 0`) or down the chain.
    pub fn move_hook(&mut self, id: &str, offset: isize) {
        let Some(from) = self.hooks.iter().position(|hook| hook == id) else {
            return;
        };
        let to = from.saturating_add_signed(offset);
        if to < self.hooks.len() && to != from {
            let hook = self.hooks.remove(from);
            self.hooks.insert(to, hook);
        }
    }

    /// Draft that would bring the profile back to a replaced history state.
    /// The builder cannot clear a slot selection, so slots missing from the
    /// old state keep their current module; modules configured now but absent
    /// from the old state get an explicit empty object.
    pub fn from_revision(snapshot: &ConfigBuilderSnapshot, state: &ConfigBuilderState) -> Self {
        let mut draft = Self::from_snapshot(snapshot);
        for selection in &state.active_modules {
            draft
                .modules
                .insert(selection.slot.clone(), selection.id.clone());
        }
        let mut config = state.module_config.clone();
        for (slot, modules) in &snapshot.module_config {
            let target = config.entry(slot.clone()).or_default();
            for module in modules.keys() {
                target
                    .entry(module.clone())
                    .or_insert_with(|| Value::Object(Map::new()));
            }
        }
        draft.config = config;
        draft.hooks = state.hooks.clone();
        draft.tools = state.tools_enabled.iter().cloned().collect();
        draft.provider = state.active_provider.clone();
        draft.mode = state.permission_mode.clone();
        draft.addons = super::addons::normalized(state.addon_settings.clone());
        draft
    }

    /// Profile areas that differ from `baseline`, in a stable order.
    pub fn areas(&self, baseline: &Draft) -> Vec<Area> {
        let request = self.changes(baseline);
        let mut areas = BTreeSet::new();
        if request.active_provider.is_some() {
            areas.insert(Area::Provider);
        }
        if request.permission_mode.is_some() {
            areas.insert(Area::Mode);
        }
        if request.tools_enabled.is_some() {
            areas.insert(Area::Tools);
        }
        if request.hooks.is_some() {
            areas.insert(Area::Hooks);
        }
        let (before, after) = (&baseline.addons, &self.addons);
        if before.mcp_servers != after.mcp_servers
            || before.addons.disabled_mcp_servers != after.addons.disabled_mcp_servers
        {
            areas.insert(Area::Mcp);
        }
        if before.addons.disabled_skills != after.addons.disabled_skills {
            areas.insert(Area::Skills);
        }
        if before.addons.plugins != after.addons.plugins {
            areas.insert(Area::Packages);
        }
        for slot in request.modules.keys().chain(request.module_config.keys()) {
            areas.insert(Area::Slot(slot.clone()));
        }
        areas.into_iter().collect()
    }

    /// Minimal save request: only sections that differ from `baseline`.
    /// The server replaces a whole slot map in `module_config`, so a changed
    /// slot is sent in full.
    pub fn changes(&self, baseline: &Draft) -> SetConfigBuilderRequest {
        let mut request = SetConfigBuilderRequest::default();
        for (slot, module) in &self.modules {
            if baseline.modules.get(slot) != Some(module) {
                request.modules.insert(slot.clone(), module.clone());
            }
        }
        let empty = BTreeMap::new();
        for (slot, modules) in &self.config {
            let before = baseline.config.get(slot).unwrap_or(&empty);
            let after: BTreeMap<String, Value> = modules
                .iter()
                .filter(|(module, value)| before.contains_key(*module) || !is_empty_object(value))
                .map(|(module, value)| (module.clone(), value.clone()))
                .collect();
            if &after != before {
                request.module_config.insert(slot.clone(), after);
            }
        }
        if self.hooks != baseline.hooks {
            request.hooks = Some(self.hooks.clone());
        }
        if self.tools != baseline.tools {
            request.tools_enabled = Some(self.tools.iter().cloned().collect());
        }
        if self.provider != baseline.provider {
            request.active_provider = self.provider.clone();
        }
        if self.mode != baseline.mode {
            request.permission_mode = serde_json::from_value(Value::String(self.mode.clone())).ok();
        }
        if self.addons != baseline.addons {
            request.addon_settings = Some(self.addons.clone());
        }
        request
    }

    pub fn is_dirty(&self, baseline: &Draft) -> bool {
        let request = self.changes(baseline);
        !request.modules.is_empty()
            || !request.module_config.is_empty()
            || request.hooks.is_some()
            || request.tools_enabled.is_some()
            || request.active_provider.is_some()
            || request.permission_mode.is_some()
            || request.addon_settings.is_some()
    }
}

fn is_empty_object(value: &Value) -> bool {
    value.as_object().is_some_and(Map::is_empty)
}
