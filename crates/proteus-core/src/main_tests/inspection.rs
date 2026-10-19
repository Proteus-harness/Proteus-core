use super::*;
use proteus_contracts::domain::{ModuleKind, ModuleManifest};

#[test]
fn inspect_plan_reports_blocking_selection_without_starting_runtime() {
    let mut config = crate::test_model::config();
    config.modules.context = Some("missing-context".to_owned());
    let dir = tempfile::tempdir().expect("workspace");
    let (plan, _) = resolve_cli_assembly(&config, None, dir.path(), config.permissions.mode)
        .expect("diagnostic plan");

    let rendered = render_inspect_plan(&plan, InspectPlanFormat::Text).expect("render plan");
    assert!(rendered.contains("состояние: запуск заблокирован"));
    assert!(rendered.contains("active module is not registered: context/missing-context"));
    assert!(plan.ensure_valid().is_err());
}

#[test]
fn inspect_topology_reports_invalid_backend_without_building_it() {
    let mut config = crate::test_model::config();
    config.modules.context = Some("missing-context".to_owned());

    let snapshot = build_cli_topology(
        &config,
        None,
        std::path::Path::new("."),
        config.permissions.mode,
    )
    .expect("best-effort topology snapshot");

    assert!(snapshot.slots.iter().any(|slot| slot.id == "context"));
    assert!(snapshot.warnings.iter().any(|warning| {
        warning
            .message
            .contains("active module is not registered: context/missing-context")
    }));
}

#[test]
fn read_only_cli_paths_do_not_start_unrelated_process_components() {
    let dir = tempfile::tempdir().expect("workspace");
    let marker = dir.path().join("process-search-started");
    let preparation_marker = dir.path().join("process-preparation-started");
    let workflow_marker = dir.path().join("process-workflow-started");
    let mut config = crate::test_model::config();
    config.modules.workflow = Some("workflow-marker".to_owned());
    config.modules.hooks = vec!["preparation-marker".to_owned()];
    config.agent_control.surface = proteus_core::core::AgentControlSurface::None;
    config.tools.enabled = vec!["request_user_input".to_owned()];
    config.components = serde_json::from_value(serde_json::json!({
        "workflow-fixture": {
            "command": "/bin/sh",
            "args": ["-c", format!("touch {}", workflow_marker.display())],
            "handshake_timeout_ms": 1000,
            "exports": {"workflow": {"workflow-marker": {}}}
        },
        "search-fixture": {
            "command": "/bin/sh",
            "args": ["-c", format!("touch {}", marker.display())],
            "exports": {"context": {"search-marker": {"timeout_ms": 1000}}}
        },
        "preparation-fixture": {
            "command": "/bin/sh",
            "args": ["-c", format!("touch {}", preparation_marker.display())],
            "exports": {"hook": {"preparation-marker": {"timeout_ms": 1000}}}
        }
    }))
    .expect("component configs");
    config
        .components
        .extend(crate::test_model::config().components);

    for provider in [config.active_provider.clone(), None] {
        config.active_provider = provider;
        let (plan, catalog) =
            resolve_cli_assembly(&config, None, dir.path(), config.permissions.mode)
                .expect("assembly plan");
        let _tools = build_tool_registry_for_listing(&plan, &catalog).expect("tool list registry");
        let topology = build_cli_topology(&config, None, dir.path(), config.permissions.mode)
            .expect("topology");
        assert!(!topology.warnings.iter().any(|warning| {
            warning
                .message
                .contains("could not build model-hosted tools")
        }));
        let mut findings = DoctorFindings::default();
        check_model_config(&mut findings, &config);
        check_external_commands(&mut findings, &config, dir.path());
        assert!(!findings.has_errors());
        for component in ["search-fixture", "preparation-fixture", "workflow-fixture"] {
            let expected = format!("процесс модуля {component} (реализаций слотов: 1)");
            assert!(
                findings
                    .entries
                    .iter()
                    .any(|entry| { entry.level == "ok" && entry.message.contains(&expected) }),
                "{expected}"
            );
        }
    }

    for marker in [marker, preparation_marker, workflow_marker] {
        assert!(
            !marker.exists(),
            "read-only CLI path unexpectedly spawned process: {}",
            marker.display()
        );
    }
}

#[test]
fn module_list_output_contains_catalog_rows() {
    let manifests = vec![ModuleManifest::builtin(
        "rg",
        ModuleKind::Tool,
        &["workspace", "ripgrep"],
    )];
    let rendered = render_module_list(&manifests);

    assert!(rendered.contains("kind"));
    assert!(rendered.contains("tool"));
    assert!(rendered.contains("rg"));
    assert!(rendered.contains("workspace,ripgrep"));
}

#[test]
fn tool_list_output_contains_registered_tools() {
    let mut config = crate::test_model::config();
    crate::test_model::add_direct_patch_tool(&mut config);
    crate::test_model::add_search_tool(&mut config);
    // Verify process-provided editing/search and the host user-input transport.
    config.tools.enabled = vec![
        "apply_patch".to_owned(),
        "search".to_owned(),
        "request_user_input".to_owned(),
    ];
    let dir = tempfile::tempdir().expect("temp dir");
    let (plan, catalog) =
        resolve_cli_assembly(&config, None, dir.path(), config.permissions.mode).unwrap();
    let registry = build_tool_registry_for_listing(&plan, &catalog).unwrap();
    let rendered = render_tool_list(&registry);

    assert!(rendered.contains("name"));
    assert!(rendered.contains("apply_patch"));
    assert!(rendered.contains("process:test-patch/direct_patch"));
    assert!(rendered.contains("process:test-search/rg_search"));
    assert!(rendered.contains("builtin:builtin"));
    assert!(rendered.contains("WritesFiles"));
    assert!(rendered.contains("search"));
    assert!(rendered.contains("ReadOnly"));
}
