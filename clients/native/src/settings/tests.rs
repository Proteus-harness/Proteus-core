use proteus_contracts::app_protocol::config_builder::{
    ConfigBuilderModule, ConfigBuilderModuleSelection, ConfigBuilderSlot, ConfigBuilderSnapshot,
    ConfigBuilderState, ConfigBuilderTool, ConfigBuilderToolPack,
};
use proteus_contracts::domain::{ConfigField, ConfigValueSchema, ModuleConfigSchema};
use serde_json::json;

use super::catalog::{
    PackState, PageId, agent_pages, module_info, pack_summary, page_title, plural,
};
use super::draft::Area;
use super::history::describe_changes;
use super::{Draft, FieldPath, TextFormat, format_value, parse_text};

fn integer(minimum: Option<i64>, maximum: Option<i64>) -> TextFormat {
    TextFormat::Integer { minimum, maximum }
}

#[test]
fn text_values_round_trip_and_validate() {
    assert_eq!(
        parse_text(integer(Some(1), Some(10)), " 7 "),
        Ok(Some(json!(7)))
    );
    assert_eq!(
        parse_text(integer(Some(1), None), "0"),
        Err("Не меньше 1".to_string())
    );
    assert_eq!(
        parse_text(integer(None, None), "x"),
        Err("Нужно целое число".to_string())
    );
    assert_eq!(parse_text(TextFormat::Line, "   "), Ok(None));
    assert_eq!(
        parse_text(TextFormat::Lines, "a\n\n b \n"),
        Ok(Some(json!(["a", "b"])))
    );
    assert_eq!(format_value(TextFormat::Lines, &json!(["a", "b"])), "a\nb");
    assert_eq!(parse_text(TextFormat::Json, "null"), Ok(None));
    assert!(parse_text(TextFormat::Json, "{").is_err());
}

#[test]
fn changes_send_only_changed_sections() {
    let mut baseline = Draft::default();
    baseline.modules.insert("context".into(), "a".into());
    baseline
        .config
        .entry("context".into())
        .or_default()
        .insert("a".into(), json!({"limit": 1, "keep": true}));
    baseline.hooks = vec!["hook.one".into()];
    baseline.mode = "normal".into();

    let mut draft = baseline.clone();
    assert!(!draft.is_dirty(&baseline));

    let limit = FieldPath::new("context", "a", "limit");
    draft.set_value(&limit, Some(json!(5)));
    draft.set_value(&FieldPath::new("workflow", "b", "x"), None);
    draft.set_module("workflow", "b");
    draft.set_hook("hook.one", false);
    draft.mode = "auto".into();

    let request = draft.changes(&baseline);
    assert_eq!(
        request.modules.get("workflow").map(String::as_str),
        Some("b")
    );
    assert!(!request.modules.contains_key("context"));
    assert_eq!(
        request
            .module_config
            .get("context")
            .and_then(|slot| slot.get("a")),
        Some(&json!({"limit": 5, "keep": true}))
    );
    assert!(!request.module_config.contains_key("workflow"));
    assert_eq!(request.hooks, Some(Vec::new()));
    assert!(request.tools_enabled.is_none());
    assert!(request.permission_mode.is_some());

    draft.set_value(&limit, Some(json!(1)));
    draft.set_module("workflow", "a");
    draft.set_module("workflow", "b");
    assert!(draft.changes(&baseline).module_config.is_empty());
}

#[test]
fn reset_leaves_an_empty_module_object_out_of_the_request() {
    let baseline = Draft::default();
    let mut draft = baseline.clone();
    let path = FieldPath::new("tool", "memory", "path");
    draft.set_value(&path, Some(json!("x.jsonl")));
    assert!(draft.is_dirty(&baseline));
    draft.set_value(&path, None);
    assert!(!draft.is_dirty(&baseline));
}

fn module(id: &str, slot: &str, fields: Vec<ConfigField>) -> ConfigBuilderModule {
    ConfigBuilderModule {
        id: id.into(),
        slot: slot.into(),
        active: true,
        source: String::new(),
        version: String::new(),
        api_version: String::new(),
        capabilities: Vec::new(),
        description: None,
        config_schema: Some(ModuleConfigSchema { fields }),
    }
}

fn slot(id: &str, order: u32, modules: Vec<ConfigBuilderModule>) -> ConfigBuilderSlot {
    ConfigBuilderSlot {
        id: id.into(),
        title: id.to_uppercase(),
        responsibility: String::new(),
        active_module: modules.first().map(|module| module.id.clone()),
        required: true,
        category: String::new(),
        order,
        modules,
    }
}

fn tool(name: &str, runtime_managed: bool) -> ConfigBuilderTool {
    ConfigBuilderTool {
        name: name.into(),
        source: "process".into(),
        owner: None,
        safety: "ReadOnly".into(),
        description: String::new(),
        enabled: true,
        runtime_managed,
        registered: true,
    }
}

#[test]
fn pages_cover_known_slots_and_append_unknown_ones() {
    let trigger = ConfigField::new("trigger", "Порог", "", ConfigValueSchema::integer(1));
    let snapshot = ConfigBuilderSnapshot {
        slots: vec![
            slot(
                "workflow",
                2,
                vec![module("codex", "workflow", vec![trigger])],
            ),
            slot("later", 9, Vec::new()),
            slot("earlier", 7, Vec::new()),
        ],
        ..ConfigBuilderSnapshot::default()
    };
    let pages = agent_pages(&snapshot);
    let slot_pages: Vec<&str> = pages
        .iter()
        .filter_map(|page| match page {
            PageId::Slot(slot) => Some(slot.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        slot_pages,
        ["workflow", "context", "policy", "earlier", "later"]
    );
    for page in [
        PageId::Model,
        PageId::ContextProviders,
        PageId::Tools,
        PageId::Hooks,
    ] {
        assert!(pages.contains(&page));
    }
    assert_eq!(pages.last(), Some(&PageId::History));
    assert_eq!(
        page_title(Some(&snapshot), &PageId::Slot("later".into())),
        "LATER"
    );
    assert!(
        module_info(&snapshot, "workflow", "codex")
            .and_then(|info| info.schema)
            .is_some()
    );
}

#[test]
fn hooks_move_and_areas_follow_edits() {
    let mut baseline = Draft::default();
    baseline.hooks = vec!["a".into(), "b".into(), "c".into()];
    let mut draft = baseline.clone();
    draft.move_hook("c", -1);
    draft.move_hook("a", -1);
    assert_eq!(draft.hooks, ["a", "c", "b"]);
    draft.set_value(
        &FieldPath::new("model", "openai", "effort"),
        Some(json!("high")),
    );
    draft.set_tool("grep", true);
    assert_eq!(
        draft.areas(&baseline),
        [Area::Tools, Area::Hooks, Area::Slot("model".into())]
    );
}

#[test]
fn revision_restores_old_state_and_describes_it() {
    let snapshot = ConfigBuilderSnapshot {
        active_modules: vec![ConfigBuilderModuleSelection {
            slot: "context".into(),
            id: "repo".into(),
        }],
        module_config: [(
            "context".to_string(),
            [("repo".to_string(), json!({"limit": 5}))]
                .into_iter()
                .collect(),
        )]
        .into_iter()
        .collect(),
        tools_enabled: vec!["grep".into(), "shell".into()],
        permission_mode: "auto".into(),
        ..ConfigBuilderSnapshot::default()
    };
    let state = ConfigBuilderState {
        active_modules: vec![ConfigBuilderModuleSelection {
            slot: "context".into(),
            id: "simple".into(),
        }],
        tools_enabled: vec!["grep".into()],
        permission_mode: "normal".into(),
        ..ConfigBuilderState::default()
    };
    let saved = Draft::from_snapshot(&snapshot);
    let restored = Draft::from_revision(&snapshot, &state);
    assert_eq!(restored.module_config("context", "repo"), Some(&json!({})));
    let request = restored.changes(&saved);
    assert_eq!(
        request
            .module_config
            .get("context")
            .and_then(|slot| slot.get("repo")),
        Some(&json!({}))
    );
    assert_eq!(
        describe_changes(&snapshot, &restored, &saved),
        [
            "Режим прав: По правилам → Правки без вопросов",
            "Tools: + shell",
            "Context: simple → repo; параметры: limit",
        ]
    );
}

#[test]
fn pack_switch_leaves_runtime_tools_alone() {
    let snapshot = ConfigBuilderSnapshot {
        tools: vec![
            tool("grep", false),
            tool("spawn_agent", true),
            tool("shell", false),
        ],
        ..ConfigBuilderSnapshot::default()
    };
    let pack = ConfigBuilderToolPack {
        id: "pack".into(),
        tools: vec!["grep".into(), "spawn_agent".into(), "shell".into()],
    };
    let mut draft = Draft::from_snapshot(&snapshot);
    draft.set_tool("grep", true);
    let summary = pack_summary(&snapshot, &draft, &pack);
    assert_eq!(summary.state, PackState::Mixed);
    assert_eq!(summary.switchable, ["grep", "shell"]);
    assert_eq!((summary.enabled, summary.total), (2, 3));
    assert_eq!(plural(3, "пакет", "пакета", "пакетов"), "3 пакета");
    assert_eq!(plural(11, "пакет", "пакета", "пакетов"), "11 пакетов");
}
