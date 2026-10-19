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
