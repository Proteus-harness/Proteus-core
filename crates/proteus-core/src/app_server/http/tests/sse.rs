use super::*;

#[test]
fn sse_output_wraps_protocol_output_as_json_data() {
    let bytes = encode_sse_output(&StdioOutput::Response {
        id: Some("req-1".to_owned()),
        ok: true,
        output: None,
        error: None,
    });
    let text = std::str::from_utf8(&bytes).expect("utf8");

    assert!(text.starts_with("event: output\ndata: "));
    assert!(text.contains(r#""type":"response""#));
    assert!(text.contains(r#""id":"req-1""#));
    assert!(text.ends_with("\n\n"));
}

#[cfg(unix)]
#[tokio::test]
async fn stale_unopened_summary_refresh_notifies_the_http_stream() {
    use crate::{
        core::{SessionStore, list_workspace_session_summaries},
        domain::{new_session_id, new_thread_id},
        model_standard::{CanonicalMessage, MessageRole},
    };
    let (state, server, root) = test_state().await;
    let store = SessionStore::new(root.path(), root.path(), new_session_id()).unwrap();
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(MessageRole::User, "saved summary")],
        )
        .await
        .unwrap();
    assert_eq!(
        list_workspace_session_summaries(root.path(), root.path()).unwrap()[0].message_count,
        1
    );
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(
                MessageRole::Assistant,
                "external update",
            )],
        )
        .await
        .unwrap();
    let mut response = super::super::sse::sse_response(state, server.clone()).await;
    response.body_mut().frame().await.unwrap().unwrap();
    assert_eq!(
        list_workspace_session_summaries(root.path(), root.path()).unwrap()[0].message_count,
        1
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let frame = response.body_mut().frame().await.unwrap().unwrap();
            let Ok(data) = frame.into_data() else { continue };
            let text = std::str::from_utf8(&data).unwrap();
            let Some(data) = text.lines().find_map(|line| line.strip_prefix("data: ")) else { continue };
            if matches!(serde_json::from_str::<StdioOutput>(data).unwrap(), StdioOutput::Event { event }
                if matches!(*event, AppServerEvent::SessionCatalogUpdated))
                && list_workspace_session_summaries(root.path(), root.path()).unwrap()[0].message_count == 2 {
                break;
            }
        }
    }).await.expect("background catalog refresh reaches the HTTP client");
    drop(response);
    // A connection established after the refresh must reconcile a list read
    // before subscribing, without waiting for another refresh.
    let (shutdown, _) = broadcast::channel(1);
    let state = HttpAppState::new(server.clone(), shutdown, test_security()).await;
    let mut reconnected = super::super::sse::sse_response(state, server.clone()).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let frame = reconnected.body_mut().frame().await.unwrap().unwrap();
            let Ok(data) = frame.into_data() else {
                continue;
            };
            if std::str::from_utf8(&data)
                .unwrap()
                .contains("session_catalog_updated")
            {
                break;
            }
        }
    })
    .await
    .expect("completed refresh remains visible to a new connection");
    server.shutdown().await;
}
