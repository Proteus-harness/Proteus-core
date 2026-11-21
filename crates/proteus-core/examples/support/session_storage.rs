use anyhow::Result;
use proteus_core::core::{AppConfig, StorageService, configured_session_storage};
use std::path::Path;

pub fn service(cwd: &Path) -> Result<StorageService> {
    let mut config = AppConfig::default();
    let command = std::env::var_os("PROTEUS_TEST_SESSION_STORAGE_MODULE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "proteus-session-storage-module".into());
    config.modules.session_storage = Some("jsonl".into());
    config.components.insert(
        "fixture-storage".into(),
        serde_json::from_value(serde_json::json!({
            "command": command, "transport": {"max_frame_bytes": 134217728},
            "exports": {"session_storage": {"jsonl": {}}},
        }))?,
    );
    configured_session_storage(&config, cwd)
}
