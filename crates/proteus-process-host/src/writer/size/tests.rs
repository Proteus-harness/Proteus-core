use serde_json::{Number, json};

use super::*;

fn assert_size(value: &Value) {
    assert_eq!(
        compact_json_len(value).unwrap(),
        serde_json::to_vec(value).unwrap().len(),
        "compact size differs for {value:?}"
    );
}

#[test]
fn every_ascii_byte_matches_for_strings_and_object_keys() {
    for byte in 0u8..=127 {
        let text = char::from(byte).to_string();
        assert_size(&Value::String(text.clone()));
        let mut object = serde_json::Map::new();
        object.insert(text.clone(), Value::String(text));
        assert_size(&Value::Object(object));
    }

    let controls: String = (0u8..=31).map(char::from).collect();
    assert_eq!(
        compact_json_len(&Value::String(controls.clone())).unwrap(),
        174
    );
    assert_size(&json!({"controls": controls, "short": "\"\\\x08\t\n\x0c\r"}));
    assert_eq!(compact_json_len(&json!("/<> &")).unwrap(), 7);
}

#[test]
fn utf8_widths_and_non_ascii_line_separators_are_not_escaped() {
    let text = "\u{7f}\u{80}\u{7ff}\u{800}\u{d7ff}\u{e000}\u{ffff}\u{10000}\u{10ffff}\u{2028}\u{2029}Привет🦀";
    assert_eq!(compact_json_len(&json!(text)).unwrap(), text.len() + 2);
    let mut object = serde_json::Map::new();
    object.insert(text.to_owned(), json!([text, "\n", "\\"]));
    assert_size(&Value::Object(object));
}

#[test]
fn containers_include_exact_punctuation_and_preserve_map_iteration() {
    for value in [
        json!(null),
        json!(true),
        json!(false),
        json!(""),
        json!([]),
        json!({}),
        json!([[], {}, [true, false, null]]),
        json!({"z": [1, "\n"], "a": {}, "": ["🦀", false]}),
        json!({"$serde_json::private::Number": "1e9999"}),
        json!({"$serde_json::private::RawValue": "[1,2]"}),
    ] {
        assert_size(&value);
    }

    let mut forward = serde_json::Map::new();
    let mut reverse = serde_json::Map::new();
    for (key, value) in [
        ("z", json!(true)),
        ("a\n", json!([1, 2])),
        ("🦀", json!(null)),
    ] {
        forward.insert(key.to_owned(), value);
    }
    for (key, value) in forward.iter().rev() {
        reverse.insert(key.clone(), value.clone());
    }
    let forward = Value::Object(forward);
    let reverse = Value::Object(reverse);
    assert_size(&forward);
    assert_size(&reverse);
    assert_eq!(
        compact_json_len(&forward).unwrap(),
        compact_json_len(&reverse).unwrap()
    );
}

#[test]
fn numeric_formatting_is_owned_by_serde_json() {
    for value in [
        json!(i64::MIN),
        json!(i64::MAX),
        json!(u64::MAX),
        json!(0),
        json!(-0.0),
        json!(f64::MIN_POSITIVE),
        json!(f64::MAX),
        json!(f64::from_bits(1)),
    ] {
        assert_size(&value);
    }
    for source in [
        "-0",
        "1.00",
        "1e+10",
        "1e-100",
        "1.2345678901234567",
        "18446744073709551616",
    ] {
        let value: Value = serde_json::from_str(source).unwrap();
        assert_size(&value);
    }
    // When arbitrary_precision is enabled by the workspace's feature graph,
    // these retain long/exponent lexemes rather than passing through f64.
    let long_integer = "1234567890".repeat(64);
    for source in ["1e9999", "-1e-9999", long_integer.as_str()] {
        if let Ok(number) = source.parse::<Number>() {
            assert_size(&Value::Number(number));
        }
    }
}

#[test]
fn mixed_trees_and_long_escape_runs_match_the_serializer() {
    let leaves = [
        json!(null),
        json!(false),
        json!(true),
        json!(17),
        json!(-12.5),
        json!("abc"),
        json!("\0\n\\\"🦀"),
        json!([]),
        json!({}),
    ];
    for index in 0..leaves.len() {
        let mut value = leaves[index].clone();
        for depth in 0..12 {
            value = if depth % 2 == 0 {
                json!([value, leaves[(index + depth) % leaves.len()]])
            } else {
                json!({"\n🦀": value, "": leaves[(index + depth) % leaves.len()]})
            };
            assert_size(&value);
        }
    }
    assert_size(&json!([
        "\0\n\"\\".repeat(4096),
        "plain UTF-8 🦀".repeat(4096)
    ]));
}

#[test]
fn overflow_is_reported_without_wrapping_or_partial_success() {
    let mut size = JsonSize { bytes: usize::MAX };
    size.add(0).unwrap();
    let error = size.add(1).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
    assert_eq!(error.to_string(), "JSON frame length overflow");
    assert_eq!(size.bytes, usize::MAX);

    for value in [
        json!(null),
        json!(false),
        json!(1),
        json!(""),
        json!([]),
        json!({}),
    ] {
        let mut size = JsonSize { bytes: usize::MAX };
        assert_eq!(
            size.value(&value).unwrap_err().to_string(),
            "JSON frame length overflow"
        );
    }
    let mut size = JsonSize {
        bytes: usize::MAX - 3,
    };
    assert!(
        size.string("\0").is_err(),
        "escape expansion must also be checked"
    );
}
