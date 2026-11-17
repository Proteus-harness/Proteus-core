use super::*;
use proteus_contracts::model_standard::{
    ModelInputEstimate, ModelInputEstimateRequest, ModelPartTokenEstimate,
};

fn estimate_request() -> ModelInputEstimateRequest {
    ModelInputEstimateRequest {
        model: ModelRef::new("external", "fixture"),
        parts: CanonicalMessage::new(
            MessageRole::Assistant,
            vec![ContentPart::Opaque {
                format: "fixture/checkpoint".into(),
                data: json!({"ciphertext":"unchanged"}),
            }],
        )
        .parts,
    }
}

#[tokio::test]
async fn dropping_input_estimate_cancels_the_process_invocation() {
    let cwd = tempfile::tempdir().unwrap();
    let marker = cwd.path().join("estimate-cancel");
    let request = estimate_request();
    let mut settings = settings();
    settings["features"] = json!(["model.input_estimate.v1"]);
    settings["expected_estimate_input"] = json!(request);
    settings["estimate_wait"] = json!(true);
    settings["estimate_marker"] = json!(marker);
    let adapter = model(&config("external", settings), cwd.path()).unwrap();
    let reader = adapter.clone();
    let lookup = tokio::spawn(async move { reader.estimate_input(request).await });
    for expected in ["started", "canceled"] {
        tokio::time::timeout(Duration::from_secs(5), async {
            while std::fs::read_to_string(&marker).ok().as_deref() != Some(expected) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        if expected == "started" {
            lookup.abort();
        }
    }
    let _ = lookup.await;
}

#[tokio::test]
async fn optional_estimate_method_preserves_exact_input_and_validates_results() {
    let cwd = tempfile::tempdir().unwrap();
    let request = estimate_request();
    let mut settings = settings();
    // Legacy model/v14 workers do not acknowledge this optional feature.
    let adapter = model(&config("external", settings.clone()), cwd.path()).unwrap();
    assert!(
        adapter
            .estimate_input(request.clone())
            .await
            .unwrap()
            .is_none()
    );
    settings["features"] = json!(["model.input_estimate.v1"]);
    settings["expected_estimate_input"] = json!(request);
    let row = ModelPartTokenEstimate {
        part_id: request.parts[0].part_id,
        tokens: 0,
    };
    let estimate = ModelInputEstimate {
        parts: vec![row.clone()],
    };
    for output in [json!(estimate), json!({"parts":[]}), Value::Null] {
        settings["input_estimate"] = output.clone();
        let adapter = model(&config("external", settings.clone()), cwd.path()).unwrap();
        assert_eq!(
            json!(adapter.estimate_input(request.clone()).await.unwrap()),
            output
        );
    }
    for invalid in [
        json!({"parts":[row.clone(),row]}),
        json!({"parts":[{"part_id":uuid::Uuid::new_v4(),"tokens":1}]}),
        json!({"parts":[{"part_id":request.parts[0].part_id,"tokens":-1}]}),
    ] {
        settings["input_estimate"] = invalid;
        let adapter = model(&config("external", settings.clone()), cwd.path()).unwrap();
        assert!(adapter.estimate_input(request.clone()).await.is_err());
    }
}
