use std::collections::{BTreeMap, BTreeSet};

use proteus_contracts::app_protocol::config_builder::ConfigBuilderSnapshot;
use proteus_contracts::app_protocol::http::SetConfigBuilderRequest;
use serde_json::{Map, Value};

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
    }
}

fn is_empty_object(value: &Value) -> bool {
    value.as_object().is_some_and(Map::is_empty)
}
