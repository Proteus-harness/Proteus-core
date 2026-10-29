use super::*;
use crate::domain::ToolSafety;
use serde_json::json;

fn spec(schema: Value) -> ToolSpec {
    ToolSpec::new("probe", "probe", schema, ToolSafety::ReadOnly)
}

#[test]
fn unchanged_schema_reuses_validator_but_changed_schema_replaces_it() {
    let cache = ToolSchemaCache::default();
    let original = spec(json!({"type": "integer", "minimum": 1}));
    let first = cache.validator(&original).unwrap();
    let mut renamed = original.clone();
    renamed.name = "other-name".into();
    renamed.description = "changed description".into();
    renamed.safety = ToolSafety::Dangerous;
    assert!(Arc::ptr_eq(&first, &cache.validator(&renamed).unwrap()));

    let changed = spec(json!({"type": "integer", "minimum": 5}));
    let second = cache.validator(&changed).unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    let call = ToolCall::new("call", "probe", json!(2));
    assert!(cache.validate_call_args(&call, &changed).is_some());
    assert!(cache.validate_call_args(&call, &original).is_none());
    // Only the current schema is retained, not a map of historical variants.
    assert!(!Arc::ptr_eq(&first, &cache.validator(&original).unwrap()));
}

#[test]
fn invalid_current_schema_never_falls_back_and_external_refs_stay_offline() {
    let cache = ToolSchemaCache::default();
    let valid = spec(json!({"type": "object"}));
    let first = cache.validator(&valid).unwrap();
    let call = ToolCall::new("call", "probe", json!({}));
    for schema in [
        json!({"type": "invalid-type"}),
        json!({"$ref": "https://example.com/tool-schema"}),
        json!({"$ref": "file:///not-a-tool-schema.json"}),
    ] {
        let invalid = spec(schema);
        assert_eq!(
            cache.validate_call_args(&call, &invalid),
            super::super::validate_tool_call_args(&call, &invalid)
        );
        assert!(cache.validate_call_args(&call, &invalid).is_some());
    }
    assert!(!Arc::ptr_eq(&first, &cache.validator(&valid).unwrap()));
}

#[test]
fn cached_validation_preserves_fresh_raw_arguments_and_error_precedence() {
    let cache = ToolSchemaCache::default();
    let spec = spec(json!({
        "$defs": {"count": {"type": "integer", "minimum": 1}},
        "type": "object", "properties": {"count": {"$ref": "#/$defs/count"}},
        "required": ["count"], "additionalProperties": false
    }));
    cache.validator(&spec).unwrap();
    for call in [
        ToolCall::new("ok", "probe", json!({"count": 2})),
        ToolCall::new("bad", "probe", json!({"count": 0})),
        ToolCall::new("raw-bad", "probe", json!({"count": 2})).with_raw_arguments("{\"count\":0}"),
        ToolCall::new("raw-good", "probe", json!({"count": 0})).with_raw_arguments("{\"count\":2}"),
        ToolCall::new("malformed", "probe", json!({"count": 2})).with_raw_arguments("{"),
    ] {
        assert_eq!(
            cache.validate_call_args(&call, &spec),
            super::super::validate_tool_call_args(&call, &spec)
        );
    }
    let invalid = self::spec(json!({"type": "invalid-type"}));
    let malformed = ToolCall::new("malformed", "probe", json!({})).with_raw_arguments("{");
    let error = cache.validate_call_args(&malformed, &invalid).unwrap();
    assert!(error.starts_with("failed to parse function arguments:"));
    assert_eq!(
        Some(error),
        super::super::validate_tool_call_args(&malformed, &invalid)
    );
}

#[test]
fn concurrent_unchanged_schema_requests_share_one_validator() {
    let cache = Arc::new(ToolSchemaCache::default());
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let handles = (0..4)
        .map(|_| {
            let cache = Arc::clone(&cache);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                cache.validator(&spec(json!({"type": "object"}))).unwrap()
            })
        })
        .collect::<Vec<_>>();
    let validators = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert!(
        validators
            .iter()
            .all(|item| Arc::ptr_eq(item, &validators[0]))
    );
}

#[test]
fn changed_map_order_keeps_current_first_error_diagnostics() {
    let cache = ToolSchemaCache::default();
    let original = spec(
        serde_json::from_str(
            r#"{"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"string"}}}"#,
        )
        .unwrap(),
    );
    let reordered = spec(
        serde_json::from_str(
            r#"{"type":"object","properties":{"b":{"type":"string"},"a":{"type":"integer"}}}"#,
        )
        .unwrap(),
    );
    let first = cache.validator(&original).unwrap();
    let call = ToolCall::new("call", "probe", json!({"a": false, "b": 1}));
    assert_eq!(
        cache.validate_call_args(&call, &reordered),
        super::super::validate_tool_call_args(&call, &reordered)
    );
    // Works with either feature set: sorted maps canonicalize both orders;
    // preserve_order maps keep them distinct, including nested properties.
    assert_eq!(
        Arc::ptr_eq(&first, &cache.validator(&reordered).unwrap()),
        original.input_schema.to_string() == reordered.input_schema.to_string()
    );
}

#[test]
fn schema_identity_preserves_numeric_spelling_and_nested_array_maps() {
    let negative_zero = json!({"const": -0.0});
    let positive_zero = json!({"const": 0.0});
    assert_eq!(
        same_schema(&negative_zero, &positive_zero),
        negative_zero.to_string() == positive_zero.to_string()
    );
    let left: Value =
        serde_json::from_str(r#"{"allOf":[{"type":"string","minLength":3}]}"#).unwrap();
    let right: Value =
        serde_json::from_str(r#"{"allOf":[{"minLength":3,"type":"string"}]}"#).unwrap();
    assert_eq!(
        same_schema(&left, &right),
        left.to_string() == right.to_string()
    );
}
