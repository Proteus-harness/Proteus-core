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

#[tokio::test]
async fn registry_releases_only_opted_in_workers_once_per_component() {
    let workspace = tempfile::tempdir().unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/process_probe_tool.py");
    let config: AppConfig = serde_json::from_value(json!({
        "tools": {"enabled": ["releasable_one", "releasable_two", "persistent_probe"]},
        "components": {
            "releasable": {
                "command": "python3",
                "args": ["-B", script],
                "release_when_idle": true,
                "exports": {"tool": {"releasable_one": {}, "releasable_two": {}}}
            },
            "persistent": {
                "command": "python3",
                "args": ["-B", script],
                "exports": {"tool": {"persistent_probe": {}}}
            }
        }
    }))
    .unwrap();
    let catalog = ModuleCatalog::from_config(&config).unwrap();
    let broker = |name| {
        catalog
            .config_exports
            .iter()
            .find(|export| export.module_id() == name)
            .unwrap()
            .connect(workspace.path())
            .unwrap()
    };
    let releasable = broker("releasable_one");
    let persistent = broker("persistent_probe");
    let registry = RuntimeRegistry::from_catalog(&config, workspace.path().to_owned(), catalog)
        .expect("tool discovery and registry assembly");
    let before = releasable.snapshot().unwrap();
    let persistent_before = persistent.snapshot().unwrap();
    assert!(before.pid.is_some() && persistent_before.pid.is_some());

    assert_eq!(registry.release_idle_component_workers().await.unwrap(), 1);
    let after = releasable.snapshot().unwrap();
    assert_eq!(after.pid, None);
    assert_eq!(after.generation, before.generation + 1);
    assert_eq!(persistent.snapshot().unwrap(), persistent_before);
    assert!(registry.tools.get("releasable_one").is_some());
    assert!(registry.tools.get("releasable_two").is_some());
    assert_eq!(registry.release_idle_component_workers().await.unwrap(), 0);
    assert!(registry.config_schemas().await.errors.is_empty());
    assert!(releasable.snapshot().unwrap().pid.is_some());
}
