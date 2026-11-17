//! Explicit process-model fixture. No production registration or implicit fallback.
use proteus_core::core::AppConfig;
use std::{path::PathBuf, sync::OnceLock};

pub fn reference_module() -> PathBuf {
    static REFERENCE_MODULE_PATH: OnceLock<PathBuf> = OnceLock::new();
    REFERENCE_MODULE_PATH
        .get_or_init(|| {
            let path = PathBuf::from(std::env::var_os("PROTEUS_TEST_REFERENCE_MODULE")
                .expect("use scripts/test.py to build and bind the reference module before running core tests"));
            assert!(path.is_file(), "reference test module is missing: {}", path.display());
            path
        })
        .clone()
}

#[allow(dead_code)]
fn storage_module() -> PathBuf {
    std::env::var_os("PROTEUS_TEST_SESSION_STORAGE_MODULE")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("PROTEUS_TEST_MODULE_BINARIES").and_then(|json| {
                let map: std::collections::HashMap<String, PathBuf> =
                    serde_json::from_str(json.to_str()?).ok()?;
                map.get("proteus-session-storage-module")
                    .or_else(|| map.get("session-storage"))
                    .cloned()
            })
        })
        .expect("use scripts/test.py to build and bind the session storage module")
}

#[allow(dead_code)]
pub fn storage_config(config: &mut AppConfig) {
    config.modules.session_storage = Some("jsonl".into());
    config.components.insert(
        "test-session-storage".into(),
        serde_json::from_value(serde_json::json!({
            "command": storage_module(), "exports": {"session_storage": {"jsonl": {}}},
            "transport": {"max_frame_bytes": 134217728},
        }))
        .unwrap(),
    );
}

/// Explicitly bind persisted child sessions without replacing the fixture's other slots.
#[allow(dead_code)]
pub fn storage_config_file(path: &std::path::Path) {
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let root = config.as_table_mut().unwrap();
    root.entry("modules")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .unwrap()
        .insert("session_storage".into(), "jsonl".into());
    let component: toml::Value = toml::from_str(&format!(
        "command = {}\n[exports.session_storage.jsonl]\n[transport]\nmax_frame_bytes = 134217728\n",
        serde_json::to_string(&storage_module()).unwrap()
    ))
    .unwrap();
    root.entry("components")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .unwrap()
        .insert("test-session-storage".into(), component);
    std::fs::write(path, toml::to_string(&config).unwrap()).unwrap();
}

#[allow(dead_code)]
pub fn storage(cwd: &std::path::Path) -> proteus_core::core::StorageService {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex, Weak},
    };
    static SERVICES: OnceLock<
        Mutex<HashMap<PathBuf, Weak<dyn proteus_contracts::contracts::SessionStorage>>>,
    > = OnceLock::new();
    let cwd = cwd.canonicalize().unwrap();
    let mut services = SERVICES.get_or_init(Default::default).lock().unwrap();
    if let Some(service) = services.get(&cwd).and_then(Weak::upgrade) {
        return service;
    }
    let mut config = AppConfig::default();
    storage_config(&mut config);
    let service = proteus_core::core::configured_session_storage(&config, &cwd).unwrap();
    services.insert(cwd, Arc::downgrade(&service));
    service
}

#[allow(dead_code)]
pub fn config() -> AppConfig {
    let mut config = AppConfig::default();
    storage_config(&mut config);
    config.components.insert(
        "test-model".into(),
        serde_json::from_value(serde_json::json!({
            "command": reference_module(), "exports": {"model": {"fake": {}}}
        }))
        .unwrap(),
    );
    config
        .module_config
        .entry("model".into())
        .or_default()
        .insert(
            "fake".into(),
            serde_json::json!({"implementation": "fake", "stream_delay_ms": 1}),
        );
    config
}

#[allow(dead_code)]
pub fn add_direct_patch_tool(config: &mut AppConfig) {
    config.components.insert(
        "test-patch".into(),
        serde_json::from_value(serde_json::json!({
            "command": reference_module(), "exports": {"tool": {"direct_patch": {}}}
        }))
        .unwrap(),
    );
}

#[allow(dead_code)]
pub fn add_search_tool(config: &mut AppConfig) {
    config.components.insert(
        "test-search".into(),
        serde_json::from_value(serde_json::json!({
            "command": reference_module(), "exports": {"tool": {"rg_search": {}}}
        }))
        .unwrap(),
    );
}

#[allow(dead_code)]
pub fn add_allow_all_policy(config: &mut AppConfig) {
    config.modules.policy = Some("allow_all".into());
    config.components.insert(
        "test-policy".into(),
        serde_json::from_value(serde_json::json!({
            "command": reference_module(), "exports": {"policy": {"allow_all": {}}}
        }))
        .unwrap(),
    );
}

#[allow(dead_code)]
pub fn toml_component() -> String {
    format!(
        "\n[components.test-model]\ncommand = {}\n[components.test-model.exports.model.fake]\n[module_config.model.fake]\nimplementation = \"fake\"\n",
        serde_json::to_string(&reference_module()).unwrap()
    )
}

#[allow(dead_code)]
pub fn no_context_preparation()
-> std::sync::Arc<dyn proteus_contracts::contracts::ContextPreparation> {
    struct Unchanged;
    #[async_trait::async_trait]
    impl proteus_contracts::contracts::ContextPreparation for Unchanged {
        async fn prepare(
            &self,
            input: proteus_contracts::contracts::ContextPreparationInput,
        ) -> anyhow::Result<proteus_contracts::contracts::ContextPreparationResult> {
            Ok(proteus_contracts::contracts::ContextPreparationResult {
                output: proteus_contracts::contracts::ContextPreparationOutput::unchanged(
                    input.messages,
                ),
                target: None,
            })
        }
    }
    std::sync::Arc::new(Unchanged)
}
