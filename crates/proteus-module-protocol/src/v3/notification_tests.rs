use super::*;

use serde::ser::{SerializeSeq, Serializer};
use serde_json::json;

#[test]
fn payload_size_matches_compact_json_bytes_without_mutation() {
    let controls = (0_u8..32).map(char::from).collect::<String>();
    let mut nested = json!({"quote\"\\\n": ["😀", null, false, 1.5, {"nested": "é"}]});
    for _ in 0..64 {
        nested = json!([nested]);
    }
    let mut ordered = serde_json::Map::new();
    ordered.insert("z-first".to_owned(), json!("line\n"));
    ordered.insert("a-second".to_owned(), json!("中"));
    for value in [
        Value::Null,
        json!(true),
        json!(i64::MIN),
        json!(u64::MAX),
        json!(-0.0),
        json!(1.25e-100),
        json!(controls),
        json!("Кириллица 中文 😀 \"\\\n\r\t"),
        json!({"seq": 0, "value": "large\n中😀".repeat(16_384)}),
        Value::Object(ordered),
        nested,
    ] {
        let encoded = serde_json::to_vec(&value).unwrap();
        assert_eq!(serialized_payload_len(&value).unwrap(), encoded.len());
        assert_eq!(serde_json::to_vec(&value).unwrap(), encoded);
    }
}

struct FailsAfterPrefix;

impl serde::Serialize for FailsAfterPrefix {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(2))?;
        sequence.serialize_element("already written")?;
        Err(serde::ser::Error::custom(
            "intentional serialization failure",
        ))
    }
}

#[test]
fn serialization_errors_do_not_return_partial_lengths() {
    let expected = serde_json::to_vec(&FailsAfterPrefix).unwrap_err();
    let actual = serialized_payload_len(&FailsAfterPrefix).unwrap_err();
    assert_eq!(actual.classify(), expected.classify());
    assert_eq!(actual.to_string(), expected.to_string());
    assert_eq!(
        serialized_payload_len(&FailsAfterPrefix).unwrap_or(usize::MAX),
        usize::MAX,
    );
}

#[test]
fn byte_budget_is_held_after_receive_until_notification_drop() {
    let payload = json!({"seq": 0, "value": "中😀\"\\\n"});
    let bytes = serialized_payload_len(&payload).unwrap();
    let (sink, mut receiver) = NotificationSink::channel(ReceiveLimits::new(8, bytes));
    let send = || sink.try_send("module.progress".to_owned(), payload.clone(), bytes);
    assert!(matches!(send(), NotificationDelivery::Delivered));
    let held = receiver.try_recv().unwrap();
    assert_eq!(held.params, payload);
    assert!(matches!(send(), NotificationDelivery::Dropped));
    assert_eq!(sink.dropped.load(Ordering::Acquire), 1);
    drop(held);
    assert!(matches!(send(), NotificationDelivery::Delivered));
    drop(receiver.try_recv().unwrap());
    drop(receiver);
    assert!(matches!(send(), NotificationDelivery::Dropped));
    assert_eq!(sink.dropped.load(Ordering::Acquire), 2);
    let state = sink.budget.state.lock().unwrap();
    assert_eq!(state.frames, 0);
    assert_eq!(state.bytes, 0);
}

#[test]
fn byte_counter_accepts_full_slices_and_preserves_capacity_overflow() {
    let mut counter = JsonByteCounter(0);
    assert_eq!(counter.write("中😀".as_bytes()).unwrap(), 7);
    assert_eq!(counter.0, 7);
    assert_eq!(counter.write(b"").unwrap(), 0);
    assert!(counter.flush().is_ok());

    let mut counter = JsonByteCounter(isize::MAX as usize - 1);
    assert_eq!(counter.write(b"x").unwrap(), 1);
    assert_eq!(counter.0, isize::MAX as usize);
    assert_eq!(counter.write(b"").unwrap(), 0);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| counter.write(b"x")));
    assert!(result.is_err());
    assert_eq!(counter.0, isize::MAX as usize);

    let mut counter = JsonByteCounter(usize::MAX);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| counter.write(b"x")));
    assert!(result.is_err());
    assert_eq!(counter.0, usize::MAX);
}
