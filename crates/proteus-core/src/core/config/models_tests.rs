use super::*;
use crate::core::{AppConfig, config_reference};
use serde_json::json;

fn source() -> Value {
    json!({
        "model": "primary",
        "models": {
            "primary": {"provider": "fake", "model": "large", "effort": "xhigh", "summary": true, "budget_tokens": 123},
            "budget/preview": {"provider": "fake", "model": "small", "effort": "high", "stream": false}
        },
        "components": {"shared": {"command": "synthetic-worker", "exports": {
            "model": {"fake": {}}, "tool": {"dangerous": {}}
        }}},
        "module_config": {"model": {"fake": {"implementation": "fake"}}, "tool": {"dangerous": {"write": true}}},
        "permissions": {"mode": "auto"},
        "tools": {"enabled": ["write_file"]},
        "agent_control": {"roles": [
            {"name": "inherit", "description": "Current parent model", "config": {"tools": {"enabled": ["read_file"]}}},
            {"name": "fixed", "description": "Named model", "config": {"model": "budget/preview"}},
            {"name": "role-pick", "description": "Role source choice", "model": "budget/preview", "config": {}}
        ]}
    })
}

async fn load(value: &Value) -> (tempfile::TempDir, std::path::PathBuf, AppConfig) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent.toml");
    std::fs::write(&path, toml::to_string(value).unwrap()).unwrap();
    let config = AppConfig::load(Some(&path)).await.unwrap();
    (directory, path, config)
}

#[tokio::test]
async fn names_are_declared_once_and_resolve_in_root_and_inline_roles_without_rights_inheritance() {
    let mut value = source();
    value["permissions"]["mode"] = json!("full_access");
    let (_directory, path, root) = load(&value).await;
    assert_eq!(
        root.permissions.mode,
        crate::domain::PermissionMode::FullAccess
    );
    let model = root.active_model_config().unwrap();
    assert_eq!(model.model, "large");
    assert_eq!(model.reasoning.effort.as_deref(), Some("xhigh"));
    assert_eq!(model.reasoning.budget_tokens, Some(123));
    assert_eq!(root.agent_control.model_presets.len(), 2);
    assert!(
        root.agent_control.roles[0].model.is_none(),
        "bootstrap is not an explicit role choice"
    );
    assert_eq!(
        root.agent_control.roles[1].model.as_ref().unwrap().model,
        "small"
    );
    assert!(!root.agent_control.roles[2].model.as_ref().unwrap().stream);

    let inherited = AppConfig::load(Some(&config_reference(&path, "inherit").unwrap()))
        .await
        .unwrap();
    assert_eq!(inherited.active_model_config().unwrap().model, "large");
    assert_eq!(
        inherited.permissions.mode,
        crate::domain::PermissionMode::Normal
    );
    assert_eq!(inherited.tools.enabled, ["read_file"]);
    assert!(inherited.modules.workflow.is_none());
    let component = serde_json::to_value(&inherited.components["shared"]).unwrap();
    assert!(component["exports"]["model"]["fake"].is_object());
    assert!(component["exports"].get("tool").is_none());
    assert_eq!(
        inherited.module_config["model"]["fake"]["implementation"],
        "fake"
    );
    assert!(!inherited.module_config.contains_key("tool"));
    let fixed = AppConfig::load(Some(&config_reference(&path, "fixed").unwrap()))
        .await
        .unwrap();
    assert_eq!(fixed.active_model_config().unwrap().model, "small");
    assert_eq!(
        fixed
            .active_model_config()
            .unwrap()
            .reasoning
            .effort
            .as_deref(),
        Some("high")
    );
    assert!(!fixed.active_model_config().unwrap().stream);
}

#[tokio::test]
async fn nested_roles_share_scoped_models_and_can_add_a_local_name() {
    let mut value = source();
    let inherited = &mut value["agent_control"]["roles"][0]["config"];
    inherited["models"] = json!({"test-run": {"provider": "fake", "model": "local", "reasoning": {"summary": true}, "effort": "low"}});
    inherited["agent_control"] = json!({"roles": [{"name": "deep", "description": "nested", "config": {"model": "test-run"}}]});
    let (_directory, path, root) = load(&value).await;
    assert!(root.agent_control.roles[0].model.is_none());
    let child_ref = config_reference(&path, "inherit").unwrap();
    let child = AppConfig::load(Some(&child_ref)).await.unwrap();
    assert_eq!(child.agent_control.model_presets.len(), 3);
    let deep = AppConfig::load(Some(&config_reference(&child_ref, "deep").unwrap()))
        .await
        .unwrap();
    assert_eq!(deep.active_model_config().unwrap().model, "local");
    assert!(deep.active_model_config().unwrap().reasoning.summary);
}

#[tokio::test]
async fn existing_explicit_provider_child_keeps_its_own_catalogue_and_model() {
    let mut value = source();
    value["agent_control"]["roles"][1]["config"] = json!({
        "active_provider": "independent", "providers": {"independent": {"provider": "fake", "model": "own"}}
    });
    let (_directory, path, root) = load(&value).await;
    assert_eq!(
        root.agent_control.roles[1].model.as_ref().unwrap().model,
        "own"
    );
    let child = AppConfig::load(Some(&config_reference(&path, "fixed").unwrap()))
        .await
        .unwrap();
    assert_eq!(child.providers.len(), 1);
    assert!(child.providers.contains_key("independent"));
    assert_eq!(child.active_model_config().unwrap().model, "own");
}

#[test]
fn unknown_names_ambiguous_sources_and_malformed_presets_fail_before_selection() {
    let mut cases = Vec::new();
    let mut unknown = source();
    unknown["model"] = "absent".into();
    cases.push(unknown);
    let mut unknown_role = source();
    unknown_role["agent_control"]["roles"][1]["config"]["model"] = "absent".into();
    cases.push(unknown_role);
    let mut ambiguous = source();
    ambiguous["active_provider"] = "primary".into();
    cases.push(ambiguous);
    let mut double_catalog = source();
    double_catalog["providers"] = json!({});
    cases.push(double_catalog);
    let mut unknown_field = source();
    unknown_field["models"]["primary"]["magical"] = true.into();
    cases.push(unknown_field);
    let mut double_effort = source();
    double_effort["models"]["primary"]["reasoning"] = json!({"effort": "low"});
    cases.push(double_effort);
    let mut empty = source();
    empty["models"]["primary"]["model"] = "".into();
    cases.push(empty);
    let mut role_conflict = source();
    role_conflict["agent_control"]["roles"][1]["model"] = "primary".into();
    cases.push(role_conflict);
    let mut object_role_conflict = source();
    object_role_conflict["agent_control"]["roles"][1]["model"] = json!({
        "provider": "fake", "model": "explicit-object", "stream": true, "reasoning": {"effort": "high"}
    });
    cases.push(object_role_conflict);
    for mut value in cases {
        assert!(normalize_model_sources(&mut value).is_err(), "{value}");
    }
}

#[test]
fn shared_transport_does_not_replace_an_explicit_child_route() {
    let mut value = source();
    value["agent_control"]["roles"][0]["config"]["components"] = json!({
        "local": {"command": "local-worker", "exports": {"model": {"fake": {}}}}
    });
    value["agent_control"]["roles"][0]["config"]["module_config"] =
        json!({"model": {"fake": {"implementation": "local"}}});
    normalize_model_sources(&mut value).unwrap();
    let child = &value["agent_control"]["roles"][0]["config"];
    assert_eq!(child["components"]["local"]["command"], "local-worker");
    assert!(child["components"].get("shared").is_none());
    assert_eq!(
        child["module_config"]["model"]["fake"]["implementation"],
        "local"
    );
}

#[test]
fn inline_role_choice_resolves_in_its_child_catalogue() {
    let mut inline_choice = source();
    inline_choice["agent_control"]["roles"] = json!([{
        "name": "local", "description": "local model",
        "config": {
            "model": "child-only",
            "models": {"child-only": {"provider": "fake", "model": "local-small", "effort": "low"}}
        }
    }]);
    let mut role_choice = inline_choice.clone();
    let role = &mut role_choice["agent_control"]["roles"][0];
    let choice = role["config"]
        .as_object_mut()
        .unwrap()
        .remove("model")
        .unwrap();
    role["model"] = choice;
    normalize_model_sources(&mut inline_choice).unwrap();
    normalize_model_sources(&mut role_choice).unwrap();
    assert_eq!(role_choice, inline_choice);
    assert_eq!(
        role_choice["agent_control"]["roles"][0]["model"]["model"],
        "local-small"
    );
}

#[test]
fn external_role_choice_still_requires_a_parent_alias() {
    let mut value = source();
    value["agent_control"]["roles"] = json!([{
        "name": "external", "description": "external model", "config": "external.toml",
        "model": "child-only"
    }]);
    assert!(
        normalize_model_sources(&mut value)
            .unwrap_err()
            .to_string()
            .contains("unknown model alias")
    );
}

#[test]
fn inline_role_choice_keeps_child_shadowing_of_parent_alias() {
    let mut value = source();
    value["agent_control"]["roles"] = json!([{
        "name": "shadow", "description": "local shadow", "model": "primary",
        "config": {"models": {"primary": {"provider": "fake", "model": "local-small"}}}
    }]);
    normalize_model_sources(&mut value).unwrap();
    assert_eq!(
        value["agent_control"]["roles"][0]["model"]["model"],
        "local-small"
    );
    assert_eq!(value["providers"]["primary"]["model"], "large");
}
