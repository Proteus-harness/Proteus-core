use proteus_contracts::app_protocol::config_builder::{
    ConfigBuilderModule, ConfigBuilderModuleSelection, ConfigBuilderSlot, ConfigBuilderSnapshot,
    ConfigBuilderTool,
};
use proteus_contracts::domain::{ConfigField, ConfigValueSchema, ModuleConfigSchema};
use serde_json::json;

use super::{Draft, FieldPath, Item, TextFormat, format_value, pages, parse_text, search};

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
    draft.set_value(&FieldPath::new("compactor", "b", "x"), None);
    draft.set_module("compactor", "b");
    draft.set_hook("hook.one", false);
    draft.mode = "auto".into();

    let request = draft.changes(&baseline);
    assert_eq!(
        request.modules.get("compactor").map(String::as_str),
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
    assert!(!request.module_config.contains_key("compactor"));
    assert_eq!(request.hooks, Some(Vec::new()));
    assert!(request.tools_enabled.is_none());
    assert!(request.permission_mode.is_some());

    draft.set_value(&limit, Some(json!(1)));
    draft.set_module("compactor", "a");
    draft.set_module("compactor", "b");
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

#[test]
fn pages_follow_slot_order_and_selected_module() {
    let trigger = ConfigField::new("trigger", "Порог", "", ConfigValueSchema::integer(1));
    let verbose =
        ConfigField::new("verbose", "Подробно", "", ConfigValueSchema::Boolean {}).advanced();
    let snapshot = ConfigBuilderSnapshot {
        slots: vec![
            ConfigBuilderSlot {
                id: "compactor".into(),
                title: "Compactor".into(),
                responsibility: String::new(),
                active_module: Some("codex".into()),
                required: true,
                category: String::new(),
                order: 2,
                modules: vec![
                    module("codex", "compactor", vec![trigger, verbose]),
                    module("plain", "compactor", Vec::new()),
                ],
            },
            ConfigBuilderSlot {
                id: "workflow".into(),
                title: "Workflow".into(),
                responsibility: "Loop".into(),
                active_module: None,
                required: true,
                category: String::new(),
                order: 0,
                modules: Vec::new(),
            },
            ConfigBuilderSlot {
                id: "unknown".into(),
                title: "Unknown".into(),
                responsibility: String::new(),
                active_module: None,
                required: false,
                category: String::new(),
                order: 9,
                modules: Vec::new(),
            },
        ],
        active_modules: vec![ConfigBuilderModuleSelection {
            slot: "compactor".into(),
            id: "codex".into(),
        }],
        tools: vec![ConfigBuilderTool {
            name: "spawn_agent".into(),
            source: "runtime".into(),
            owner: None,
            safety: String::new(),
            description: String::new(),
            enabled: true,
            runtime_managed: true,
            registered: true,
        }],
        ..ConfigBuilderSnapshot::default()
    };
    let mut draft = Draft::from_snapshot(&snapshot);
    let baseline = draft.clone();
    let layout: Vec<_> = pages(&snapshot, &draft)
        .into_iter()
        .map(|page| (page.id, page.groups.len()))
        .collect();
    assert_eq!(
        layout,
        vec![
            ("context".to_string(), 1),
            ("tools".to_string(), 1),
            ("workflow".to_string(), 1),
            ("unknown".to_string(), 1),
        ]
    );
    let all = pages(&snapshot, &draft);
    let compactor = &all[0].groups[0];
    assert_eq!(compactor.title.as_deref(), Some("Сжатие истории"));
    assert!(matches!(compactor.items[0], Item::Module { .. }));
    assert_eq!(compactor.items.len(), 2);
    assert_eq!(compactor.advanced.len(), 1);
    assert!(matches!(
        all[1].groups[0].items[0],
        Item::Tool { locked: true, .. }
    ));
    assert!(matches!(all[2].groups[0].items[0], Item::Note(_)));
    assert_eq!(all[3].title, "Unknown");

    let found = search(&all, "подробно");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].title.as_deref(), Some("Контекст · Сжатие истории"));

    draft.set_module("compactor", "plain");
    let all = pages(&snapshot, &draft);
    assert_eq!(all[0].groups[0].items.len(), 1);
    assert!(all[0].changed(&draft, &baseline));
    assert!(!all[1].changed(&draft, &baseline));
}
