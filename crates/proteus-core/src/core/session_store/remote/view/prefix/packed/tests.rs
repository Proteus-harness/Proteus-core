use super::*;
use serde_json::json;

fn fixture() -> PrefixIdentities {
    let known = Uuid::from_u128(100);
    PrefixIdentities {
        history: vec![[12; 32], [1; 32], [12; 32]],
        turns: HashSet::from([Uuid::from_u128(9), known]),
        exchanges: HashSet::from([known]),
        messages: HashSet::from([Uuid::from_u128(2), known]),
        parts: HashMap::from([(known, true), (Uuid::from_u128(4), false)]),
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
    for v in [new] {
        let mut read = PrefixIdentities::decode_cache(v).unwrap();
        assert_eq!(read.history, original.history);
        assert_eq!(read.turns, original.turns);
        assert_eq!(read.exchanges, original.exchanges);
        assert_eq!(read.messages, original.messages);
        assert_eq!(read.parts, original.parts);
        assert!(read.has_explicit_part(&Uuid::from_u128(100)));
        assert!(!read.has_explicit_part(&Uuid::from_u128(4)));
        assert!(!read.has_explicit_part(&Uuid::from_u128(101)));
        assert_eq!(read.calls, original.calls);
        let mut message =
            CanonicalMessage::text(crate::model_standard::MessageRole::User, "suffix");
        assert!(read.message(&message, true, false));
        message.parts[0].part_id = Uuid::from_u128(4);
        assert!(
            !read.message(&message, true, false),
            "non-explicit known part still fences append"
        );
    }
    let empty = PrefixIdentities::default();
    let rebuilt = PrefixIdentities::decode_cache(value(&empty)).unwrap();
    assert!(rebuilt.calls.is_empty() && rebuilt.history.is_empty() && rebuilt.parts.is_empty());
}

#[test]
fn invalid_tag_encoding_membership_or_duplicate_is_a_cache_miss() {
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
        // Same UUID with opposing flag still fails the whole certificate.
        (
            "parts",
            json!(STANDARD.encode({
                let mut rows = [0; 34];
                rows[33] = 1;
                rows
            })),
        ),
        ("surprise", json!(true)),
    ] {
        let mut v = empty.clone();
        v[field] = bad;
        assert!(PrefixIdentities::decode_cache(v).is_none(), "{field}");
    }
    let mut no_messages = empty.clone();
    no_messages.as_object_mut().unwrap().remove("messages");
    assert!(PrefixIdentities::decode_cache(no_messages).is_none());
    let old = json!({
        "history": [], "turns": [], "exchanges": [], "messages": [],
        "parts": [Uuid::from_u128(4)], "explicit_committed_parts": [], "calls": [],
    });
    assert!(PrefixIdentities::decode_cache(old.clone()).is_none());
    let mut hybrid = old;
    hybrid["format"] = json!(0);
    assert!(PrefixIdentities::decode_cache(hybrid).is_none());
}

#[test]
fn over_budget_fields_are_not_used() {
    let mut v = value(&PrefixIdentities::default());
    v["history"] = Value::String("A".repeat(LIMIT + 1));
    assert!(PrefixIdentities::decode_cache(v).is_none());
}

#[test]
fn observations_can_become_explicit_but_never_undo_a_commit() {
    let mut prefix = PrefixIdentities::default();
    let message = CanonicalMessage::text(crate::model_standard::MessageRole::User, "suffix");
    let id = message.parts[0].part_id;
    assert!(prefix.message(&message, false, false));
    assert!(!prefix.has_explicit_part(&id));
    assert!(!prefix.visit_history_message(&message, true));
    assert!(
        !prefix.has_explicit_part(&id),
        "suffix checking is read only"
    );
    assert!(prefix.visit_history_message(&message, false));
    assert!(prefix.has_explicit_part(&id));
    assert!(prefix.message(&message, false, false));
    assert!(prefix.has_explicit_part(&id));
    assert!(
        PrefixIdentities::decode_cache(value(&prefix))
            .unwrap()
            .has_explicit_part(&id)
    );
}

#[test]
fn startup_moves_original_fields_without_reordering_the_packed_value() {
    let prefix = fixture();
    let borrowed = prefix.encode_cache().unwrap().into_bounded_value().unwrap();
    let owned = prefix
        .into_encoded_cache()
        .unwrap()
        .into_bounded_value()
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&owned).unwrap(),
        serde_json::to_vec(&borrowed).unwrap()
    );
    assert!(PrefixIdentities::decode_cache(owned).is_some());
}

#[test]
fn startup_and_borrowed_reject_the_same_escaped_over_budget_prefix() {
    let mut prefix = fixture();
    // The lower bound fits, but the quotes and escaping make the actual
    // serialized value too large. Never save a value accepted by only one path.
    prefix
        .calls
        .insert(char::from(92u8).to_string().repeat(LIMIT / 2 + 1));
    assert!(
        prefix
            .encode_cache()
            .unwrap()
            .into_bounded_value()
            .is_none()
    );
    assert!(
        prefix
            .into_encoded_cache()
            .unwrap()
            .into_bounded_value()
            .is_none()
    );
}
