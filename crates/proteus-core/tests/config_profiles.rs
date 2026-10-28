use std::path::{Path, PathBuf};

use proteus_core::core::{AppConfig, ModuleCatalog};
use proteus_module_protocol::current_process_contract_authority;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[tokio::test]
async fn context_search_profile_changes_only_the_context_selection_and_configuration() {
    let root = workspace_root();
    let base = AppConfig::load(Some(&root.join("configs/config.toml")))
        .await
        .unwrap();
    let experiment = AppConfig::load(Some(
        &root.join("configs/context-search-chatgpt.config.toml"),
    ))
    .await
    .unwrap();
    assert_eq!(experiment.modules.context.as_deref(), Some("repo_aware"));
    let mut base = serde_json::to_value(base).unwrap();
    let mut experiment = serde_json::to_value(experiment).unwrap();
    // Everything outside this slot, including model limits/effort, peers, tools,
    // workflow and permissions, must remain identical to the working profile.
    for value in [&mut base, &mut experiment] {
        value["profile"].as_object_mut().unwrap().remove("name");
        value["modules"].as_object_mut().unwrap().remove("context");
        value["module_config"]
            .as_object_mut()
            .unwrap()
            .remove("context");
        value["components"]["reference-context"]["exports"]
            .as_object_mut()
            .unwrap()
            .remove("context");
    }
    assert_eq!(base, experiment);
}

#[tokio::test]
async fn subscription_profiles_keep_root_and_every_peer_on_oauth() {
    let root = workspace_root();
    for profile in ["config.toml", "codex.config.toml", "restricted.config.toml"] {
        let config = AppConfig::load(Some(&root.join("configs").join(profile)))
            .await
            .unwrap();
        let mut configs = vec![config.clone()];
        for role in &config.agent_control.roles {
            assert!(role.config.starts_with("codex-"));
            configs.push(
                AppConfig::load(Some(
                    &root.join(format!("configs/{}.config.toml", role.config)),
                ))
                .await
                .unwrap(),
            );
        }
        assert_eq!(configs.len(), 3);
        for config in configs {
            assert_eq!(config.providers.len(), 1);
            assert_eq!(config.active_provider.as_deref(), Some("chatgpt"));
            assert_eq!(config.providers["chatgpt"].provider, "openai_codex");
            let settings = config
                .process_export_config("model", "openai_codex")
                .unwrap();
            assert_eq!(settings["implementation"], "openai_codex");
            assert!(
                !settings
                    .as_object()
                    .unwrap()
                    .keys()
                    .any(|key| key.starts_with("api_key"))
            );
        }
    }
}

#[tokio::test]
async fn restricted_profile_preserves_default_behavior_and_trusted_capabilities() {
    let root = workspace_root();
    let default = AppConfig::load(Some(&root.join("configs/config.toml")))
        .await
        .unwrap();
    let restricted = AppConfig::load(Some(&root.join("configs/restricted.config.toml")))
        .await
        .unwrap();

    // Includes merge export maps deeply: an accidental trusted component include
    // would retain old exports and silently duplicate the restricted selection.
    let exports = |config: &AppConfig| {
        let mut exports = config
            .components
            .values()
            .flat_map(|component| component.exports())
            .map(|(slot, module, _)| (slot.to_owned(), module.to_owned()))
            .collect::<Vec<_>>();
        exports.sort();
        exports
    };
    assert_eq!(exports(&default), exports(&restricted));
    assert_eq!(
        serde_json::to_value(&default.components["reference-model"]).unwrap(),
        serde_json::to_value(&restricted.components["reference-model"]).unwrap()
    );
    let mut expected_capabilities =
        serde_json::to_value(&default.components["reference-capabilities"]).unwrap();
    for slot in ["policy", "hook"] {
        expected_capabilities["exports"]
            .as_object_mut()
            .unwrap()
            .remove(slot);
    }
    expected_capabilities["exports"]["tool"]
        .as_object_mut()
        .unwrap()
        .remove("rg_search");
    expected_capabilities["exports"]["context_provider"]
        .as_object_mut()
        .unwrap()
        .remove("rg_search");
    assert_eq!(
        serde_json::to_value(&restricted.components["reference-capabilities"]).unwrap(),
        expected_capabilities
    );
    for profile in ["config.toml", "codex.config.toml"] {
        let trusted = AppConfig::load(Some(&root.join("configs").join(profile)))
            .await
            .unwrap();
        assert!(trusted.components.values().all(|component| {
            serde_json::to_value(component)
                .unwrap()
                .get("sandbox")
                .is_none()
        }));
    }
    let mut default = serde_json::to_value(default).unwrap();
    let mut restricted = serde_json::to_value(restricted).unwrap();
    for config in [&mut default, &mut restricted] {
        config.as_object_mut().unwrap().remove("components");
        config["profile"].as_object_mut().unwrap().remove("name");
    }
    assert_eq!(default, restricted);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn restricted_profile_resolves_six_single_export_read_only_process_specs() {
    use proteus_core::process_adapters::ProcessComponentConfig;
    use serde_json::json;

    let config = AppConfig::load(Some(
        &workspace_root().join("configs/restricted.config.toml"),
    ))
    .await
    .unwrap();
    let state = tempfile::tempdir().unwrap();
    let workspace = state.path().join("workspace");
    let bin = state.path().join("bin");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&bin).unwrap();
    // Resolve the shipped command against a private child PATH. No mutation of
    // the test process environment or dependency on an installed reference binary.
    std::fs::copy(
        std::env::current_exe().unwrap(),
        bin.join("proteus-reference-module"),
    )
    .unwrap();
    let workspace = workspace.canonicalize().unwrap();
    let restricted = [
        ("reference-workflow", "workflow", "coding.codex_loop", false),
        ("reference-context", "context", "codex_context", true),
        ("reference-policy", "policy", "codex_policy", false),
        ("reference-compact", "hook", "codex_compact", false),
        ("reference-search-tool", "tool", "rg_search", true),
        (
            "reference-search-provider",
            "context_provider",
            "rg_search",
            true,
        ),
    ];
    assert_eq!(
        config
            .components
            .values()
            .filter(|component| {
                serde_json::to_value(component).unwrap()["sandbox"]["mode"] == "restricted"
            })
            .count(),
        restricted.len()
    );
    for (id, slot, module, workspace_access) in restricted {
        let component = &config.components[id];
        assert_eq!(component.command(), "proteus-reference-module");
        assert_eq!(
            component
                .exports()
                .map(|(slot, module, _)| (slot, module))
                .collect::<Vec<_>>(),
            [(slot, module)]
        );
        let mut value = serde_json::to_value(component).unwrap();
        assert_eq!(value["env_allowlist"], json!([]));
        assert_eq!(value["env"], json!({}));
        value["env"]["PATH"] = json!(bin);
        let component: ProcessComponentConfig = serde_json::from_value(value).unwrap();
        let spec = component.process_spec(&workspace).unwrap();
        assert_eq!(spec.command, "proteus-reference-module");
        assert!(spec.args.is_empty());
        assert_eq!(
            spec.resolved_environment()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["PATH"]
        );
        assert_eq!(
            spec.cwd.as_deref(),
            Some(if workspace_access {
                workspace.as_path()
            } else {
                Path::new("/usr")
            })
        );
        let sandbox = spec.sandbox.unwrap();
        assert!(sandbox.read_executable);
        assert!(!sandbox.network);
        assert!(sandbox.read_write.is_empty());
        let mut expected = ["/usr", "/lib", "/lib64", "/bin"]
            .map(PathBuf::from)
            .to_vec();
        if workspace_access {
            expected.push(workspace.join("."));
        }
        assert_eq!(sandbox.read_only, expected);
    }
}

fn tracked_configs() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut files = Vec::new();
    for directory in [root.join("configs"), root.join("examples/configs")] {
        for entry in std::fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()))
        {
            let path = entry.expect("config directory entry").path();
            if path.is_file()
                && matches!(
                    path.extension().and_then(|extension| extension.to_str()),
                    Some("toml" | "json")
                )
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[tokio::test]
async fn tracked_profiles_resolve_selected_exports_with_current_contracts() {
    let files = tracked_configs();
    assert!(!files.is_empty(), "expected tracked config examples");

    for path in files {
        let config = AppConfig::load(Some(&path))
            .await
            .unwrap_or_else(|error| panic!("failed to load {}: {error:#}", path.display()));
        let catalog = ModuleCatalog::from_config(&config).unwrap_or_else(|error| {
            panic!("failed to build catalog for {}: {error:#}", path.display())
        });

        for profile in config.providers.values() {
            let manifest = catalog
                .manifest(
                    proteus_contracts::domain::ModuleKind::Model,
                    &profile.provider,
                )
                .unwrap_or_else(|| {
                    panic!(
                        "{} selects missing model export {}",
                        path.display(),
                        profile.provider
                    )
                });
            assert_eq!(
                manifest.api_version,
                current_process_contract_authority("model")
                    .expect("model authority")
                    .contract_version
            );
            let settings = config
                .process_export_config("model", &profile.provider)
                .unwrap();
            assert!(
                settings["implementation"].is_string(),
                "{} must explicitly choose a reference model implementation",
                path.display()
            );
        }

        for (kind, module_id) in config.modules.iter() {
            let manifest = catalog.manifest(kind, module_id).unwrap_or_else(|| {
                panic!(
                    "{} selects missing catalog module {}/{module_id}",
                    path.display(),
                    kind.as_str()
                )
            });
            let expected_version = current_process_contract_authority(kind.as_str())
                .unwrap_or_else(|| panic!("missing process authority for {}", kind.as_str()))
                .contract_version;
            assert_eq!(
                manifest.api_version,
                expected_version,
                "{} must select the current process contract for {}/{module_id}",
                path.display(),
                kind.as_str()
            );
        }
    }
}
