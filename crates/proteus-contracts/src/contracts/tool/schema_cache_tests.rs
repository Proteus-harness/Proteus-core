use super::*;
use crate::domain::ToolSafety;
use serde_json::{Value, json};

struct ChangingTool(Arc<Mutex<ToolSpec>>);

#[async_trait]
impl Tool for ChangingTool {
    fn spec(&self) -> ToolSpec {
        self.0.lock().unwrap().clone()
    }

    async fn invoke(&self, call: &ToolCall, _ctx: ToolContext) -> Result<ToolResult> {
        Ok(ToolResult::ok(
            call.id.clone(),
            "not invoked by these tests",
        ))
    }
}

fn registry(schema: Value) -> (ToolRegistry, Arc<Mutex<ToolSpec>>) {
    let state = Arc::new(Mutex::new(ToolSpec::new(
        "probe",
        "original",
        schema,
        ToolSafety::ReadOnly,
    )));
    let mut registry = ToolRegistry::new();
    registry.register(ChangingTool(Arc::clone(&state))).unwrap();
    (registry, state)
}

#[test]
fn reselection_shares_compilation_but_does_not_freeze_dynamic_specs() {
    let (source, state) = registry(json!({"type": "integer", "minimum": 1}));
    let original = source.spec("probe").unwrap();
    let cache = &source.schema_caches["probe"];
    let compiled = cache.validator(&original).unwrap();
    let cloned = source.clone();
    assert!(Arc::ptr_eq(cache, &cloned.schema_caches["probe"]));

    let mut selected = ToolRegistry::new();
    selected.register_from(&source, "probe").unwrap();
    assert!(Arc::ptr_eq(cache, &selected.schema_caches["probe"]));
    assert!(Arc::ptr_eq(&compiled, &cache.validator(&original).unwrap()));

    {
        let mut current = state.lock().unwrap();
        current.input_schema = json!({"type": "integer", "minimum": 5});
        current.description = "current".into();
        current.safety = ToolSafety::Dangerous;
    }
    let current = selected.spec("probe").unwrap();
    assert_eq!(current.description, "current");
    assert_eq!(current.safety, ToolSafety::Dangerous);
    let call = ToolCall::new("call", "probe", json!(2));
    assert!(selected.validate_call_args(&call, &current).is_some());
    assert!(selected.validate_call_args(&call, &original).is_none());

    state.lock().unwrap().input_schema = json!({"type": "invalid-type"});
    let mut invalid_selection = ToolRegistry::new();
    assert!(invalid_selection.register_from(&source, "probe").is_err());
    assert!(invalid_selection.get("probe").is_none());
    assert!(
        selected
            .validate_call_args(&call, &selected.spec("probe").unwrap())
            .is_some()
    );
}

#[test]
fn independent_registrations_do_not_share_by_name_and_duplicates_still_fail() {
    let (source, _) = registry(json!({"type": "integer"}));
    let (other, _) = registry(json!({"type": "string"}));
    assert!(!Arc::ptr_eq(
        &source.schema_caches["probe"],
        &other.schema_caches["probe"]
    ));
    let call = ToolCall::new("call", "probe", json!(2));
    assert!(
        source
            .validate_call_args(&call, &source.spec("probe").unwrap())
            .is_none()
    );
    assert!(
        other
            .validate_call_args(&call, &other.spec("probe").unwrap())
            .is_some()
    );

    let mut selected = ToolRegistry::new();
    selected.register_from(&source, "probe").unwrap();
    assert!(selected.register_from(&other, "probe").is_err());
    assert!(
        selected
            .validate_call_args(&call, &selected.spec("probe").unwrap())
            .is_none()
    );
    assert!(selected.register_from(&source, "missing").is_err());
}

#[test]
fn unknown_name_keeps_standalone_validation_behavior() {
    let (registry, _) = registry(json!({"type": "integer"}));
    let spec = ToolSpec::new(
        "unregistered",
        "test",
        json!({"type": "string"}),
        ToolSafety::ReadOnly,
    );
    let call = ToolCall::new("call", "unregistered", json!(1));
    assert_eq!(
        registry.validate_call_args(&call, &spec),
        crate::domain::validate_tool_call_args(&call, &spec)
    );
}
