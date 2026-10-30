use super::*;

#[tokio::test]
async fn unacknowledged_optional_methods_are_not_called_and_stream_still_works() {
    let cwd = tempfile::tempdir().unwrap();
    for features in [
        json!([]),
        json!(["model.catalog.v1"]),
        json!(["model.quota.v1"]),
    ] {
        let mut settings = settings();
        settings["features"] = features.clone();
        // A call to an unacknowledged method would fail decoding, rather than
        // accidentally pass this test by returning the same null result.
        settings["catalog"] =
            if features.as_array().unwrap().is_empty() || features[0] == "model.quota.v1" {
                json!({"not_a_catalog":true})
            } else {
                Value::Null
            };
        settings["quota"] =
            if features.as_array().unwrap().is_empty() || features[0] == "model.catalog.v1" {
                json!({"not_a_quota":true})
            } else {
                Value::Null
            };
        let adapter = model(&config("minimal", settings), cwd.path()).unwrap();
        assert!(adapter.catalog().await.unwrap().is_none());
        assert!(adapter.quota().await.unwrap().is_none());
        assert_eq!(
            adapter.complete(request("minimal")).await.unwrap(),
            response()
        );
    }
}

#[tokio::test]
async fn acknowledged_metadata_errors_cannot_masquerade_as_missing_features() {
    let cwd = tempfile::tempdir().unwrap();
    let mut settings = settings();
    settings["metadata_errors"] = json!({
        "catalog": "UnsupportedFeature: model.catalog.v1 was not acknowledged",
        "quota": "UnsupportedFeature: model.quota.v1 was not acknowledged"
    });
    let adapter = model(&config("claimed", settings), cwd.path()).unwrap();
    let catalog_error = adapter.catalog().await.unwrap_err();
    let quota_error = adapter.quota().await.unwrap_err();
    assert!(format!("{catalog_error:#}").contains("UnsupportedFeature"));
    assert!(format!("{quota_error:#}").contains("UnsupportedFeature"));
    assert_eq!(
        adapter.complete(request("claimed")).await.unwrap(),
        response()
    );
}
