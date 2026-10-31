#![cfg(unix)]
use super::*;
use crate::model_standard::{CanonicalMessage, ContentPart, MessageRole};

#[tokio::test]
async fn latest_tool_state_http_validates_query_and_distinguishes_not_ready_from_absent() {
    let (state, server, root) = test_state().await;
    let store =
        crate::core::SessionStore::new(root.path(), root.path(), crate::domain::new_session_id())
            .unwrap();
    store
        .append_history(
            crate::domain::new_thread_id(),
            None,
            &[
                CanonicalMessage::new(
                    MessageRole::Assistant,
                    vec![ContentPart::ToolCall {
                        call: ToolCall::new("compact", "opaque / exact", json!({"steps":[]})),
                    }],
                ),
                CanonicalMessage::new(
                    MessageRole::Tool,
                    vec![ContentPart::ToolResult {
                        result: crate::domain::ToolResult::ok("compact".into(), "SECRET_OUTPUT")
                            .with_metadata(json!({"SECRET_METADATA":true})),
                    }],
                ),
            ],
        )
        .await
        .unwrap();
    let base = format!(
        "/history/tool-state?session_dir={}",
        store.session_dir().display()
    );
    let journal = std::fs::read(store.journal_path()).unwrap();
    for suffix in [
        "",
        "&name=",
        "&name=%00",
        "&name=x&name=y",
        "&name=x&unknown=y",
        "&name=%FF",
    ] {
        let response = route_request(
            state.clone(),
            authed_get_request(&format!("{base}{suffix}")),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{suffix}");
    }
    let query = format!("{base}&name=opaque%20%2F%20exact");
    let response = route_request(state.clone(), authed_get_request(&query))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(!store.session_dir().join(".read-view.sqlite").exists());
    store
        .project_read_model(|source| {
            crate::app_server::journal_transcript_from_source(source, None)
        })
        .unwrap();
    let response = route_request(state.clone(), authed_get_request(&query))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response_bytes(response).await;
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["tool"]["call_id"], "compact");
    assert_eq!(value["tool"]["args"], json!({"steps":[]}));
    assert_eq!(value["tool"].as_object().unwrap().len(), 5);
    assert!(
        !String::from_utf8(bytes.to_vec())
            .unwrap()
            .contains("SECRET")
    );
    let response = route_request(
        state.clone(),
        authed_get_request(&format!("{base}&name=missing")),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value = serde_json::from_slice(&response_bytes(response).await).unwrap();
    assert!(value["tool"].is_null());
    assert!(
        value["revision"]
            .as_str()
            .is_some_and(|revision| !revision.is_empty())
    );
    assert!(
        state
            .server_for_session_dir(store.session_dir())
            .await
            .is_none()
    );
    assert_eq!(std::fs::read(store.journal_path()).unwrap(), journal);
    std::fs::write(store.journal_path(), b"{corrupt complete record}\n").unwrap();
    let corrupt = std::fs::read(store.journal_path()).unwrap();
    let response = route_request(state.clone(), authed_get_request(&query))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(std::fs::read(store.journal_path()).unwrap(), corrupt);
    let response = route_request(
        state,
        authed_get_request("/history/tool-state?session_dir=relative&name=x"),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    server.shutdown().await;
}
