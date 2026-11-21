use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{
    AppConfig, ConfigInputs, config_reference, config_selection, physical_config_path,
    selected_config_value_mut,
};

fn source() -> Value {
    json!({
        "profile": {"name": "codex"},
        "active_provider": "parent", "providers": {"parent": {"provider": "fake", "model": "parent-model"}},
        "tools": {"enabled": ["write_file"]},
        "agent_control": {"roles": [{
            "name": "explore", "description": "independent read-only child", "parallel_safe": true,
            "config": {
                "active_provider": "child", "providers": {"child": {"provider": "fake", "model": "child-model"}},
                "tools": {"enabled": ["read_file"]},
                "instructions": [{"kind": "System", "text": "inline child prompt", "priority": 100}]
            }
        }]}
    })
}

fn write(path: &Path, value: &Value) {
    std::fs::write(path, toml::to_string(value).unwrap()).unwrap();
}

#[tokio::test]
async fn monolithic_inline_child_is_independent_and_moves_without_generated_config_files() {
    let fixture = tempfile::tempdir().unwrap();
    let before = fixture.path().join("before");
    std::fs::create_dir(&before).unwrap();
    write(&before.join("agent.toml"), &source());
    let moved = fixture.path().join("moved");
    std::fs::rename(&before, &moved).unwrap();
    let path = moved.join("agent.toml");
    let parent = AppConfig::load(Some(&path)).await.unwrap();
    assert_eq!(parent.profile.name, "codex");
    assert!(
        parent.modules.workflow.is_none(),
        "profile name is only a label"
    );
    assert_eq!(parent.tools.enabled, ["write_file"]);
    let reference = PathBuf::from(&parent.agent_control.roles[0].config);
    assert_eq!(physical_config_path(&reference), path);
    assert_eq!(config_selection(&reference).unwrap(), ["explore"]);
    let child = AppConfig::load(Some(&reference)).await.unwrap();
    assert_eq!(child.tools.enabled, ["read_file"]);
    assert_eq!(child.active_model_config().unwrap().model, "child-model");
    assert!(!child.providers.contains_key("parent"));
    assert!(child.agent_control.roles.is_empty());
    assert_eq!(
        child.instructions[0].text.as_deref(),
        Some("inline child prompt")
    );
    assert_eq!(std::fs::read_dir(&moved).unwrap().count(), 1);
}

#[tokio::test]
async fn nested_inline_roles_resolve_prompts_against_the_original_source_and_watch_them() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("agent.toml");
    let prompt = fixture.path().join("nested.md");
    std::fs::write(&prompt, "nested file prompt").unwrap();
    let mut value = source();
    let child = &mut value["agent_control"]["roles"][0]["config"];
    child["agent_control"] = json!({"roles": [{"name": "a/b~c", "description": "nested", "config": {
        "instructions": [{"kind": "System", "file": "nested.md", "priority": 100}]
    }}]});
    write(&path, &value);
    let mut inputs = ConfigInputs::watching();
    let root = AppConfig::load_observed(Some(&path), &mut inputs)
        .await
        .unwrap();
    assert!(inputs.is_current());
    let child = AppConfig::load(Some(Path::new(&root.agent_control.roles[0].config)))
        .await
        .unwrap();
    let nested_ref = PathBuf::from(&child.agent_control.roles[0].config);
    assert_eq!(config_selection(&nested_ref).unwrap(), ["explore", "a/b~c"]);
    let nested = AppConfig::load(Some(&nested_ref)).await.unwrap();
    assert_eq!(
        nested.instructions[0].text.as_deref(),
        Some("nested file prompt")
    );
    std::fs::write(&prompt, "new nested file prompt").unwrap();
    assert!(
        !inputs.is_current(),
        "root validation observes nested prompt files"
    );
}

#[tokio::test]
async fn physical_source_changes_invalidate_inline_child_observation() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("agent.toml");
    write(&path, &source());
    let reference = config_reference(&path, "explore").unwrap();
    let mut inputs = ConfigInputs::watching();
    AppConfig::load_observed(Some(&reference), &mut inputs)
        .await
        .unwrap();
    assert!(inputs.is_current());
    let mut changed = source();
    changed["agent_control"]["roles"][0]["config"]["tools"]["enabled"] = json!(["read_many_files"]);
    write(&path, &changed);
    assert!(!inputs.is_current());
    let child = AppConfig::load(Some(&reference)).await.unwrap();
    assert_eq!(child.tools.enabled, ["read_many_files"]);
}

#[tokio::test]
async fn invalid_inline_child_configs_are_rejected_before_any_child_spawns() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("agent.toml");
    for invalid in [
        json!({"unknown_field": true}),
        json!({"include": "hidden-fragment.toml"}),
        json!({"modules": {"hooks": ["same", "same"]}}),
        json!({"instructions": [{"kind": "System", "text": "x", "file": "x.md", "priority": 100}]}),
        json!({"instructions": [{"kind": "System", "file": "missing.md", "priority": 100}]}),
    ] {
        let mut value = source();
        value["agent_control"]["roles"][0]["config"] = invalid;
        write(&path, &value);
        assert!(AppConfig::load(Some(&path)).await.is_err());
    }
}

#[tokio::test]
async fn malformed_role_sources_and_unknown_selections_fail_explicitly() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("agent.toml");
    let mut value = source();
    value["agent_control"]["roles"][0]["config"] = json!(42);
    write(&path, &value);
    assert!(AppConfig::load(Some(&path)).await.is_err());
    for name in ["", "explore"] {
        let mut value = source();
        let mut extra = value["agent_control"]["roles"][0].clone();
        extra["name"] = json!(name);
        value["agent_control"]["roles"]
            .as_array_mut()
            .unwrap()
            .push(extra);
        write(&path, &value);
        assert!(AppConfig::load(Some(&path)).await.is_err());
    }
    write(&path, &source());
    let unknown = config_reference(&path, "missing").unwrap();
    assert!(
        AppConfig::load(Some(&unknown))
            .await
            .unwrap_err()
            .to_string()
            .contains("not found")
    );
    let mut value = source();
    value["agent_control"]["roles"][0]["config"] = json!("separate-child");
    write(&path, &value);
    let selected = config_reference(&path, "explore").unwrap();
    assert!(
        AppConfig::load(Some(&selected))
            .await
            .unwrap_err()
            .to_string()
            .contains("inline config object")
    );
}

#[test]
fn selectors_roundtrip_names_and_leave_non_reserved_hash_paths_ordinary() {
    let file = Path::new("/example/agent#copy.toml");
    assert_eq!(physical_config_path(file), file);
    assert!(config_selection(file).unwrap().is_empty());
    let selected = config_reference(file, "a/b~c").unwrap();
    assert_eq!(
        selected,
        Path::new("/example/agent#copy.toml#agent=/a~1b~0c")
    );
    assert_eq!(physical_config_path(&selected), file);
    assert_eq!(config_selection(&selected).unwrap(), ["a/b~c"]);
    let nested = config_reference(&selected, "leaf").unwrap();
    assert_eq!(config_selection(&nested).unwrap(), ["a/b~c", "leaf"]);
    for invalid in [
        "x.toml#agent=",
        "x.toml#agent=x",
        "x.toml#agent=/",
        "x.toml#agent=/x/",
        "x.toml#agent=/x~2",
        "#agent=/x",
    ] {
        assert!(config_selection(Path::new(invalid)).is_err(), "{invalid}");
    }
    let mut value = source();
    let selected = selected_config_value_mut(&mut value, &["explore".into()]).unwrap();
    selected["tools"]["enabled"] = json!(["changed"]);
    assert_eq!(value["tools"]["enabled"], json!(["write_file"]));
    assert_eq!(
        value["agent_control"]["roles"][0]["config"]["tools"]["enabled"],
        json!(["changed"])
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_read_only_monolithic_source_needs_no_temporary_child_configs() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("agent.toml");
    write(&path, &source());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let config = AppConfig::load(Some(&path)).await.unwrap();
    let child = AppConfig::load(Some(Path::new(&config.agent_control.roles[0].config)))
        .await
        .unwrap();
    assert_eq!(child.tools.enabled, ["read_file"]);
    assert_eq!(std::fs::read_dir(fixture.path()).unwrap().count(), 1);
}
