use super::*;
use crate::{
    contracts::{ConversationView, ModelContextObservation},
    core::SessionStore,
    domain::{HistoryCompactionReport, new_session_id, new_thread_id},
    model_standard::{CanonicalMessage, MessageRole},
};

async fn read_conversation(
    state: &HttpAppState,
    session_dir: &std::path::Path,
) -> ConversationView {
    let response = route_request(
        state.clone(),
        authed_get_request(&format!(
            "/conversation?session_dir={}",
            session_dir.display()
        )),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&response_bytes(response).await).unwrap()
}

#[tokio::test]
async fn conversation_read_addresses_warm_and_cold_sessions_without_resuming_or_mutating_them() {
    let (state, server, config_dir) = dogfood_loop_state().await;
    let live_dir = server.session_dir_path().unwrap();
    // Reading a brand-new live session must not materialize a journal.
    assert!(!live_dir.exists());
    let empty = read_conversation(&state, &live_dir).await;
    assert_eq!(empty.source.session_id, server.session_id());
    assert!(empty.conversation.messages.is_empty());
    assert_eq!(empty.conversation.revision, 0);
    assert!(!live_dir.exists());

    server
        .send_user_message("live conversation".to_owned())
        .await
        .unwrap();
    let live_store = SessionStore::open(live_dir.clone()).unwrap();
    let live_journal = std::fs::read(live_store.journal_path()).unwrap();
    let live = read_conversation(&state, &live_dir).await;
    assert_eq!(
        live.conversation.messages,
        server.runtime.history().await.unwrap()
    );
    assert_eq!(live.source.revision, live.conversation.revision);
    assert!(!live.conversation.messages.is_empty());
    assert_eq!(
        live.conversation,
        live_store.conversation_snapshot().unwrap()
    );
    assert_eq!(
        std::fs::read(live_store.journal_path()).unwrap(),
        live_journal
    );

    let cold_id = new_session_id();
    let cold_store = SessionStore::new(config_dir.path(), server.cwd_path(), cold_id).unwrap();
    let thread = new_thread_id();
    cold_store
        .append_history(
            thread,
            None,
            &[CanonicalMessage::text(
                MessageRole::User,
                "replaced earlier conversation",
            )],
        )
        .await
        .unwrap();
    let compacted = CanonicalMessage::text(MessageRole::User, "current compacted conversation")
        .with_metadata(json!({"retained": [1, "opaque"]}));
    let mut report = HistoryCompactionReport::unchanged(1, Some("test compaction".to_owned()));
    report.changed = true;
    cold_store
        .replace_history(thread, None, std::slice::from_ref(&compacted), Some(report))
        .await
        .unwrap();
    let cold_journal = std::fs::read(cold_store.journal_path()).unwrap();
    let cold = read_conversation(&state, cold_store.session_dir()).await;
    assert_eq!(cold.source.session_id, cold_id);
    assert_ne!(cold.source.session_id, live.source.session_id);
    assert_eq!(cold.source.revision, 2);
    assert_eq!(cold.conversation.messages, vec![compacted]);
    assert_eq!(
        cold.conversation.model_context,
        vec![ModelContextObservation::HistoryCompacted]
    );
    assert_eq!(
        std::fs::read(cold_store.journal_path()).unwrap(),
        cold_journal
    );
    assert!(
        state
            .server_for_session_dir(cold_store.session_dir())
            .await
            .is_none()
    );
    assert_eq!(read_conversation(&state, &live_dir).await, live);

    // Removing the live runtime changes the transport path, not the projection.
    server.shutdown().await;
    state.remove_session_server(&live_dir).await;
    assert_eq!(read_conversation(&state, &live_dir).await, live);
    assert_eq!(
        std::fs::read(live_store.journal_path()).unwrap(),
        live_journal
    );
}
