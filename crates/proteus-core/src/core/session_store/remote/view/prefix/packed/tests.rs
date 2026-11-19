use super::*;
use serde_json::json;

fn fixture() -> PrefixIdentities {
    let known = Uuid::from_u128(100);
    PrefixIdentities {
        history: vec![[12; 32], [1; 32], [12; 32]],
        turns: HashSet::from([Uuid::from_u128(9), known]),
        exchanges: HashSet::from([known]),
        messages: HashSet::from([Uuid::from_u128(2), known]),
        parts: HashSet::from([known, Uuid::from_u128(4)]),
        explicit_committed_parts: HashSet::from([known]),
        calls: HashSet::from([String::new(), "not/UUID: э\n✨".into()]),
    }
}

fn value(prefix: &PrefixIdentities) -> Value {
    serde_json::to_value(prefix.encode_cache().unwrap()).unwrap()
}

#[test]
fn tagged_certificate_preserves_all_exact_sets_and_hash_order() {
    let original = fixture();
    let new = value(&original);
    assert!(serde_json::from_value::<PrefixIdentities>(new.clone()).is_err());
    let old = serde_json::to_value(&original).unwrap();
    assert!(PrefixIdentities::decode_cache(old).is_none());
    for v in [new] {
        let mut read = PrefixIdentities::decode_cache(v).unwrap();
        assert_eq!(read.history, original.history);
        assert_eq!(read.turns, original.turns);
        assert_eq!(read.exchanges, original.exchanges);
        assert_eq!(read.messages, original.messages);
        assert_eq!(read.parts, original.parts);
        assert_eq!(
            read.explicit_committed_parts,
            original.explicit_committed_parts
        );
        assert_eq!(read.calls, original.calls);
        let mut message =
            CanonicalMessage::text(crate::model_standard::MessageRole::User, "suffix");
        assert!(read.message(&message, true));
        message.parts[0].part_id = Uuid::from_u128(4);
        assert!(
            !read.message(&message, true),
            "non-explicit known part still fences append"
        );
    }
    let empty = PrefixIdentities::default();
    let rebuilt = PrefixIdentities::decode_cache(value(&empty)).unwrap();
    assert!(rebuilt.calls.is_empty() && rebuilt.history.is_empty() && rebuilt.parts.is_empty());
}

#[test]
fn invalid_tag_encoding_membership_or_duplicate_is_a_cache_miss() {
    let original = fixture();
    let empty = value(&PrefixIdentities::default());
    for (field, bad) in [
        ("format", json!(2)),
        ("format", json!("1")),
        ("history", json!(STANDARD.encode([4; 31]))),
        ("turns", json!("%%%%")),
        ("messages", json!(STANDARD.encode([9; 17]))),
        ("turns", json!(STANDARD.encode([1; 32]))),
        ("calls", json!(["same", "same"])),
        ("parts", json!(STANDARD.encode([2; 17]))),
        ("parts", json!(STANDARD.encode([0; 34]))),
        ("surprise", json!(true)),
    ] {
        let mut v = empty.clone();
        v[field] = bad;
        assert!(PrefixIdentities::decode_cache(v).is_none(), "{field}");
    }
    let mut no_messages = empty.clone();
    no_messages.as_object_mut().unwrap().remove("messages");
    assert!(PrefixIdentities::decode_cache(no_messages).is_none());
    let mut hybrid = serde_json::to_value(original).unwrap();
    hybrid["format"] = json!(0);
    assert!(PrefixIdentities::decode_cache(hybrid).is_none());
}

#[test]
fn invalid_subset_and_over_budget_fields_are_not_published_or_used() {
    let mut original = fixture();
    original
        .explicit_committed_parts
        .insert(Uuid::from_u128(999));
    assert!(original.encode_cache().is_none());

    let mut v = value(&PrefixIdentities::default());
    v["history"] = Value::String("A".repeat(LIMIT + 1));
    assert!(PrefixIdentities::decode_cache(v).is_none());
}
