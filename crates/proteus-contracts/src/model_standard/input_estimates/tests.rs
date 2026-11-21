use super::*;
use crate::{
    domain::ModelRef,
    model_standard::{CanonicalMessage, MessageRole},
};
use serde_json::json;

fn request() -> ModelInputEstimateRequest {
    let message = CanonicalMessage::new(
        MessageRole::Assistant,
        vec![ContentPart::Opaque {
            format: "provider/checkpoint".into(),
            data: json!({"encrypted": "secret"}),
        }],
    );
    ModelInputEstimateRequest {
        model: ModelRef::new("selected", "model"),
        parts: message.parts,
    }
}

#[test]
fn estimates_accept_zero_partial_results_and_reject_unknown_or_duplicate_identity() {
    let request = request();
    let row = ModelPartTokenEstimate {
        part_id: request.parts[0].part_id,
        tokens: 0,
    };
    let estimate = ModelInputEstimate {
        parts: vec![row.clone()],
    };
    assert!(estimate.validate(&request).is_ok());
    assert_eq!(estimate.tokens_for(&request.parts[0]), Some(0));
    assert!(ModelInputEstimate::default().validate(&request).is_ok());
    assert!(
        ModelInputEstimate {
            parts: vec![row.clone(), row.clone()]
        }
        .validate(&request)
        .is_err()
    );
    let mut other = row;
    other.part_id = crate::domain::new_part_id();
    assert!(
        ModelInputEstimate { parts: vec![other] }
            .validate(&request)
            .is_err()
    );
    let mut request = request;
    request.parts.push(request.parts[0].clone());
    assert!(request.validate().is_err());
    request.parts = CanonicalMessage::text(MessageRole::User, "text").parts;
    assert!(request.validate().is_err());
}

#[test]
fn estimate_metadata_preserves_other_fields_and_does_not_modify_opaque_material() {
    let request = request();
    let estimate = ModelInputEstimate {
        parts: vec![ModelPartTokenEstimate {
            part_id: request.parts[0].part_id,
            tokens: 433,
        }],
    };
    let original = request.parts.clone();
    let mut metadata = json!({"owner": "unchanged"});
    assert!(estimate.write_metadata(&mut metadata));
    assert_eq!(ModelInputEstimate::from_metadata(&metadata), Some(estimate));
    assert_eq!(metadata["owner"], "unchanged");
    assert_eq!(request.parts, original);
    assert!(ContextTokenEstimateSource::Observed.write_metadata(&mut metadata));
    assert_eq!(
        ContextTokenEstimateSource::from_metadata(&metadata),
        Some(ContextTokenEstimateSource::Observed)
    );
    let mut scalar = json!(42);
    assert!(!ModelInputEstimate::default().write_metadata(&mut scalar));
    assert!(!ContextTokenEstimateSource::Estimated.write_metadata(&mut scalar));
    assert_eq!(scalar, json!(42));
}

#[test]
fn malformed_estimate_metadata_is_not_a_zero_estimate() {
    let id = crate::domain::new_part_id();
    for value in [
        json!({"parts":[{"part_id":id,"tokens":-1}]}),
        json!({"parts":[{"part_id":id,"tokens":4294967296u64}]}),
        json!({"parts":[{"part_id":id,"tokens":1},{"part_id":id,"tokens":2}]}),
    ] {
        assert!(
            ModelInputEstimate::from_metadata(&json!({INPUT_TOKEN_ESTIMATES_METADATA_KEY:value}))
                .is_none()
        );
    }
}

#[test]
fn metadata_hints_only_apply_to_matching_opaque_parts() {
    let request = request();
    let mut text = CanonicalMessage::text(MessageRole::User, "text");
    text.parts[0].part_id = request.parts[0].part_id;
    let mut estimate = ModelInputEstimate {
        parts: vec![ModelPartTokenEstimate {
            part_id: request.parts[0].part_id,
            tokens: 7,
        }],
    };
    assert_eq!(estimate.tokens_for(&text.parts[0]), None);
    estimate.retain_for(&[text]);
    assert!(estimate.parts.is_empty());
    let row = ModelPartTokenEstimate {
        part_id: request.parts[0].part_id,
        tokens: 7,
    };
    let duplicate = ModelInputEstimate {
        parts: vec![row.clone(), row],
    };
    let mut metadata = json!({"other":1});
    assert!(!duplicate.write_metadata(&mut metadata));
    assert_eq!(metadata, json!({"other":1}));
}
