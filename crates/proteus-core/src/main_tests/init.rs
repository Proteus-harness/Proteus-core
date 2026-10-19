use super::*;
use proteus_contracts::domain::ModuleKind;

#[test]
fn init_destination_uses_config_file_or_profile_file_in_dir() {
    assert_eq!(
        init_destination_path(Path::new("/tmp/config.toml"), InitProfile::Coding),
        PathBuf::from("/tmp/config.toml")
    );
    assert_eq!(
        init_destination_path(Path::new("/tmp/configs"), InitProfile::Safe),
        PathBuf::from("/tmp/configs/config.toml")
    );
}

#[test]
fn init_config_path_from_arg_expands_named_config() {
    let expected_codex_path =
        AppConfig::named_config_destination_path(Path::new("codex")).expect("codex config path");
    let expected_dev_slim_path = AppConfig::named_config_destination_path(Path::new("dev-slim"))
        .expect("dev-slim config path");
    assert_eq!(
        init_config_path_from_arg(Path::new("codex")),
        expected_codex_path
    );
    assert_eq!(
        init_config_path_from_arg(Path::new("dev-slim")),
        expected_dev_slim_path
    );
    assert_eq!(
        init_config_path_from_arg(Path::new("./codex")),
        PathBuf::from("./codex")
    );
    assert_eq!(
        init_config_path_from_arg(Path::new("codex.config.toml")),
        PathBuf::from("codex.config.toml")
    );
}

#[test]
fn mixed_config_files_warning_lists_sibling_config_files() {
    let dir = tempfile::tempdir().expect("config dir");
    let config = dir.path().join(INIT_CONFIG_FILE);
    std::fs::write(&config, "").expect("config");
    std::fs::write(dir.path().join("00-provider.toml"), "").expect("sibling provider");
    std::fs::write(dir.path().join("10-coding.toml"), "").expect("sibling profile");
    std::fs::write(dir.path().join("notes.md"), "").expect("notes");

    let warning = mixed_config_files_warning(&config).expect("warning");

    assert!(warning.contains("00-provider.toml"));
    assert!(warning.contains("10-coding.toml"));
    assert!(!warning.contains("notes.md"));
    assert!(warning.contains("--config"));
}

#[test]
fn single_config_file_for_warning_resolves_directory_config_toml() {
    let dir = tempfile::tempdir().expect("config dir");
    let config = dir.path().join(INIT_CONFIG_FILE);
    std::fs::write(&config, "").expect("config");

    assert_eq!(
        single_config_file_for_warning(Some(dir.path())),
        Some(config)
    );
}

#[tokio::test]
async fn init_coding_writes_loadable_single_config_file() {
    let dir = tempfile::tempdir().expect("config dir");

    run_init(InitProfile::Coding, Some(dir.path())).expect("init coding");

    let profile = dir.path().join(INIT_CONFIG_FILE);
    assert!(profile.exists());
    assert!(dir.path().join("prompts/direct-patch.md").exists());
    let profile_body = std::fs::read_to_string(&profile).expect("profile body");
    assert!(profile_body.starts_with("active_provider = \"anthropic\""));
    assert!(
        !profile_body
            .lines()
            .any(|line| line.trim_start().starts_with("include = "))
    );

    let config = AppConfig::load(Some(&profile))
        .await
        .expect("generated config loads");
    let model = config.active_model_config().expect("active model");

    assert_eq!(config.profile.name, "coding-local");
    assert_eq!(config.active_provider.as_deref(), Some("anthropic"));
    assert_eq!(model.provider, "anthropic");
    assert_eq!(
        config.modules.workflow.as_deref(),
        Some("coding.single_loop")
    );
}

#[tokio::test]
async fn init_codex_writes_loadable_config_with_runtime_fragment() {
    let dir = tempfile::tempdir().expect("config dir");

    run_init(InitProfile::Codex, Some(dir.path())).expect("init codex");

    let profile = dir.path().join(INIT_CONFIG_FILE);
    assert!(profile.exists());
    let profile_body = std::fs::read_to_string(&profile).expect("profile body");
    assert!(profile_body.starts_with("include = \"fragments/codex-runtime.toml\""));
    assert!(profile_body.contains("active_provider = \"anthropic\""));

    let config = AppConfig::load(Some(&profile))
        .await
        .expect("generated config loads");

    assert_eq!(config.profile.name, "codex");
    assert_eq!(config.active_provider.as_deref(), Some("anthropic"));
    assert_eq!(
        config.active_model_config().expect("active model").provider,
        "anthropic"
    );
    assert_eq!(
        config.modules.workflow.as_deref(),
        Some("coding.codex_loop")
    );
    assert_eq!(config.modules.context.as_deref(), Some("codex_context"));
    assert_eq!(
        config.process_export_config("tool", "codex_patch").unwrap()["reject_self_move"],
        true
    );
    assert!(config.modules.hooks.contains(&"codex_compact".to_owned()));
    assert_eq!(config.agent_control.roles.len(), 2);
    for role in &config.agent_control.roles {
        let path = Path::new(&role.config);
        assert!(
            path.is_absolute(),
            "generated peer must not resolve against a global profile directory"
        );
        assert_eq!(path.parent(), Some(dir.path()));
        assert!(path.exists());
    }
    assert_eq!(
        config.module_config_value(ModuleKind::Context, "codex_context")["providers"],
        serde_json::json!(["project_instructions", "skills", "environment"])
    );
    assert!(dir.path().join("prompts/codex-default.md").exists());
    assert!(dir.path().join("prompts/codex-explore.md").exists());
    assert!(dir.path().join("prompts/codex-coder.md").exists());
    assert!(dir.path().join("fragments/codex-runtime.toml").exists());
    assert!(
        dir.path()
            .join("fragments/codex-peer-runtime.toml")
            .exists()
    );
    assert!(
        dir.path()
            .join("fragments/codex-explore-peer.toml")
            .exists()
    );
    assert!(dir.path().join("fragments/codex-coder-peer.toml").exists());
    let packaged_prompt = include_str!("../../../../configs/prompts/codex-default.md");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("prompts/codex-default.md"))
            .expect("installed prompt"),
        packaged_prompt
    );
    assert!(
        config
            .instruction_blocks()
            .iter()
            .any(|block| block.text == packaged_prompt)
    );

    let explore = AppConfig::load(Some(&dir.path().join("codex-explore.config.toml")))
        .await
        .expect("generated explore peer loads");
    let coder = AppConfig::load(Some(&dir.path().join("codex-coder.config.toml")))
        .await
        .expect("generated coder peer loads");
    assert_eq!(
        explore
            .active_model_config()
            .expect("explore model")
            .provider,
        "anthropic"
    );
    assert_eq!(
        coder.active_model_config().expect("coder model").provider,
        "anthropic"
    );
    assert!(
        !explore
            .tools
            .enabled
            .iter()
            .any(|tool| tool == "write_file")
    );
    assert!(coder.tools.enabled.iter().any(|tool| tool == "write_file"));
}
