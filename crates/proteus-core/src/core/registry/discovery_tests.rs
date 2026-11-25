use std::path::Path;

use serde_json::json;

use super::*;

#[tokio::test]
async fn registry_releases_discovery_only_brokers_and_can_reconnect_for_schema() {
    let workspace = tempfile::tempdir().unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/process_probe_tool.py");
    let config: AppConfig = serde_json::from_value(json!({
        "tools": {"enabled": ["active_probe"]},
        "components": {
            "active-component": {
                "command": "python3",
                "args": ["-B", script],
                "exports": {"tool": {"active_probe": {}, "disabled_sibling": {}}}
            },
            "discovery-component": {
                "command": "python3",
                "args": ["-B", script],
                "exports": {"tool": {"discovery_probe": {}}}
            }
        }
    }))
    .unwrap();
    let catalog = ModuleCatalog::from_config(&config).unwrap();
    let export = |id| {
        catalog
            .config_exports
            .iter()
            .find(|export| export.module_id() == id)
            .unwrap()
            .clone()
    };
    let active = export("active_probe");
    let discovery = export("discovery_probe");
    let active_broker = Arc::downgrade(&active.connect(workspace.path()).unwrap());
    let discovery_broker = Arc::downgrade(&discovery.connect(workspace.path()).unwrap());

    let registry = RuntimeRegistry::from_catalog(&config, workspace.path().to_owned(), catalog)
        .expect("tool discovery and registry assembly");
    assert!(discovery_broker.upgrade().is_none());
    assert!(active_broker.upgrade().is_some());
    assert!(registry.tools.get("active_probe").is_some());
    for name in ["disabled_sibling", "discovery_probe"] {
        assert!(registry.tools.get(name).is_none());
        assert!(
            registry
                .process_tool_specs
                .iter()
                .any(|(_, spec)| spec.name == name)
        );
    }
    assert!(registry.config_schemas().await.errors.is_empty());
    let restored_broker = Arc::downgrade(&discovery.connect(workspace.path()).unwrap());
    assert!(!restored_broker.ptr_eq(&discovery_broker));
    let retained_broker = Arc::downgrade(&active.connect(workspace.path()).unwrap());
    assert!(retained_broker.ptr_eq(&active_broker));
}
