use super::*;

#[cfg(unix)]
#[tokio::test]
async fn paged_snapshot_full_history_and_revision_checked_pages_agree() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let config = root.path().join("config.toml");
    let store =
        crate::core::SessionStore::new(root.path(), cwd.path(), crate::domain::new_session_id())
            .unwrap();
    let messages: Vec<_> = (0..121)
        .map(|i| {
            crate::model_standard::CanonicalMessage::text(
                crate::model_standard::MessageRole::User,
                format!("row {i}"),
            )
        })
        .collect();
    store
        .append_history(crate::domain::new_thread_id(), None, &messages)
        .await
        .unwrap();
    let before = std::fs::read(store.journal_path()).unwrap();
    let server = AgentAppServer::launch_resumed(
        crate::test_model::config(),
        cwd.path().into(),
        Some(&config),
        store.session_dir().into(),
    )
    .await
    .unwrap();
    let mut events = server.subscribe_session();
    let snapshot = loop {
        if let AppServerEvent::SessionSnapshot { snapshot } = events.recv().await.unwrap() {
            break snapshot;
        }
    };
    assert_eq!(snapshot.transcript.len(), 50);
    assert_eq!(snapshot.transcript[0].text, "row 71");
    let cursor = snapshot.history.unwrap();
    assert_eq!(cursor.before, 71);
    let full = server.transcript().await.unwrap();
    assert_eq!(full.len(), 121);
    assert_eq!(full[0].text, "row 0");
    assert!(full.iter().all(|row| row.timestamp_ms.is_some()));
    let query = format!(
        "session_dir={}&before=71&revision={}",
        store.session_dir().display(),
        cursor.revision
    );
    let page = super::super::sessions::history_page_json(Some(&query))
        .await
        .unwrap();
    assert_eq!((page.start, page.messages.len()), (21, 50));
    // Full active-session reads and revision-bound pages must both survive a
    // disposable-index failure. Exercise each path independently: the first
    // recovery must not mask the second by repairing its cache beforehand.
    for damage in ["missing", "malformed", "checksum", "unwritable"] {
        for full_read in [true, false] {
            damage_read_index(&store, damage);
            if full_read {
                assert_eq!(
                    serde_json::to_value(server.transcript().await.unwrap()).unwrap(),
                    serde_json::to_value(&full).unwrap(),
                    "{damage}: active full history"
                );
            } else {
                let recovered = super::super::sessions::history_page_json(Some(&query))
                    .await
                    .unwrap();
                assert_eq!(recovered.revision, cursor.revision, "{damage}");
                assert_eq!((recovered.start, recovered.total), (21, 121), "{damage}");
                assert_eq!(
                    serde_json::to_value(&recovered.messages).unwrap(),
                    serde_json::to_value(&full[21..71]).unwrap(),
                    "{damage}: older page"
                );
            }
            assert_eq!(
                std::fs::read(store.journal_path()).unwrap(),
                before,
                "{damage}"
            );
        }
    }
    let query = format!(
        "session_dir={}&before=71&revision=wrong",
        store.session_dir().display()
    );
    assert!(
        super::super::sessions::history_page_json(Some(&query))
            .await
            .is_err()
    );
    let query = format!("session_dir={}&before=abc", store.session_dir().display());
    assert!(
        super::super::sessions::history_page_json(Some(&query))
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(store.journal_path()).unwrap(), before);
    server.shutdown().await;
    let query = format!(
        "session_dir={}&before=71&revision={}",
        store.session_dir().display(),
        cursor.revision
    );
    let reopened = super::super::sessions::history_page_json(Some(&query))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&reopened.messages).unwrap(),
        serde_json::to_value(&full[21..71]).unwrap()
    );
    assert_eq!(std::fs::read(store.journal_path()).unwrap(), before);
}

#[tokio::test]
async fn route_history_can_read_requested_cold_session_without_changing_live_registry() {
    let cwd = tempfile::tempdir().expect("cwd");
    let config_dir = tempfile::tempdir().expect("config dir");
    let config_path = config_dir.path().join("config.toml");
    let saved_session_id = crate::domain::new_session_id();
    let saved_store =
        crate::core::SessionStore::new(config_dir.path(), cwd.path(), saved_session_id)
            .expect("session store");
    saved_store
        .append_history(
            crate::domain::new_thread_id(),
            None,
            &[crate::model_standard::CanonicalMessage::text(
                crate::model_standard::MessageRole::User,
                "saved cold history",
            )],
        )
        .await
        .expect("append saved history");
    let server = AgentAppServer::launch(
        crate::test_model::config(),
        cwd.path().to_path_buf(),
        Some(&config_path),
    )
    .await
    .expect("app server");
    let (shutdown, _) = broadcast::channel(1);
    let state = HttpAppState::new(server.clone(), shutdown, test_security()).await;

    let response = route_request(
        state.clone(),
        authed_get_request(&format!(
            "/history?session_dir={}",
            saved_store.session_dir().display()
        )),
    )
    .await
    .expect("history response");

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response_bytes(response).await;
    let transcript: Vec<Value> = serde_json::from_slice(&bytes).expect("history JSON");
    assert_eq!(transcript.len(), 1);
    assert_eq!(
        transcript[0].get("role").and_then(Value::as_str),
        Some("user")
    );
    assert_eq!(
        transcript[0].get("text").and_then(Value::as_str),
        Some("saved cold history")
    );
    let analysis = route_request(
        state.clone(),
        authed_get_request(&format!(
            "/analysis?session_dir={}",
            saved_store.session_dir().display()
        )),
    )
    .await
    .unwrap();
    assert_eq!(analysis.status(), StatusCode::OK);
    let analysis: proteus_contracts::app_protocol::analysis::AppSessionAnalysis =
        serde_json::from_slice(&response_bytes(analysis).await).unwrap();
    assert_eq!(analysis.session_id, saved_session_id);
    assert!(analysis.turns.is_empty());
    assert!(analysis.selected.is_none());
    let live_dir = server.session_dir_path().expect("live session dir");
    assert!(!live_dir.exists());
    let analysis = route_request(
        state.clone(),
        authed_get_request(&format!("/analysis?session_dir={}", live_dir.display())),
    )
    .await
    .unwrap();
    assert_eq!(analysis.status(), StatusCode::OK);
    let analysis: proteus_contracts::app_protocol::analysis::AppSessionAnalysis =
        serde_json::from_slice(&response_bytes(analysis).await).unwrap();
    assert_eq!(analysis.session_id, server.session_id());
    assert!(analysis.turns.is_empty() && analysis.selected.is_none());
    assert!(!live_dir.exists());
    assert!(
        state
            .server_for_session_dir(saved_store.session_dir())
            .await
            .is_none()
    );
    assert!(
        state
            .server_for_session_dir(
                server
                    .session_dir_path()
                    .expect("live session dir")
                    .as_path(),
            )
            .await
            .is_some()
    );

    server.shutdown().await;
}

#[tokio::test]
async fn route_context_can_read_requested_cold_session_without_changing_live_registry() {
    let cwd = tempfile::tempdir().expect("cwd");
    let config_dir = tempfile::tempdir().expect("config dir");
    let config_path = config_dir.path().join("config.toml");
    let saved_session_id = crate::domain::new_session_id();
    let saved_store =
        crate::core::SessionStore::new(config_dir.path(), cwd.path(), saved_session_id)
            .expect("session store");
    saved_store
        .append_history(
            crate::domain::new_thread_id(),
            None,
            &[crate::model_standard::CanonicalMessage::text(
                crate::model_standard::MessageRole::User,
                "saved cold context",
            )],
        )
        .await
        .expect("append saved history");
    let server = AgentAppServer::launch(
        crate::test_model::config(),
        cwd.path().to_path_buf(),
        Some(&config_path),
    )
    .await
    .expect("app server");
    let (shutdown, _) = broadcast::channel(1);
    let state = HttpAppState::new(server.clone(), shutdown, test_security()).await;

    let response = route_request(
        state.clone(),
        authed_get_request(&format!(
            "/context?session_dir={}",
            saved_store.session_dir().display()
        )),
    )
    .await
    .expect("context response");

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response_bytes(response).await;
    let snapshot: Value = serde_json::from_slice(&bytes).expect("context JSON");
    assert_eq!(
        snapshot
            .pointer("/history/messages")
            .and_then(Value::as_u64),
        Some(1)
    );
    let expected_session_id = saved_session_id.to_string();
    assert_eq!(
        snapshot.get("session_id").and_then(Value::as_str),
        Some(expected_session_id.as_str())
    );
    assert!(
        snapshot
            .get("diagnostics")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    );
    assert!(
        state
            .server_for_session_dir(saved_store.session_dir())
            .await
            .is_none()
    );
    assert!(
        state
            .server_for_session_dir(
                server
                    .session_dir_path()
                    .expect("live session dir")
                    .as_path(),
            )
            .await
            .is_some()
    );

    server.shutdown().await;
}

#[tokio::test]
async fn live_history_matches_cold_journal_after_turn_settlement() {
    let (state, server, _config_dir) = dogfood_loop_state().await;
    server
        .send_user_message("hello".to_owned())
        .await
        .expect("settled turn");
    let session_dir = server.session_dir_path().expect("session directory");
    let query = format!("session_dir={}", session_dir.display());
    let live = super::super::sessions::history_json(&state, Some(&query))
        .await
        .expect("live history");
    assert!(live.iter().any(|message| message.role == "assistant"));
    assert!(live.iter().all(|message| message.timestamp_ms.is_some()));
    let journal_path = session_dir.join("journal.jsonl");
    let journal = std::fs::read(&journal_path).unwrap();

    server.shutdown().await;
    state.remove_session_server(&session_dir).await;
    let cold = super::super::sessions::history_json(&state, Some(&query))
        .await
        .expect("cold history");
    assert_eq!(
        serde_json::to_value(live).unwrap(),
        serde_json::to_value(cold).unwrap()
    );
    assert_eq!(std::fs::read(journal_path).unwrap(), journal);
}

#[cfg(unix)]
fn damage_read_index(store: &crate::core::SessionStore, damage: &str) {
    let path = store.session_dir().join(".read-view.sqlite");
    if path.is_dir() {
        std::fs::remove_dir(&path).unwrap();
    } else if path.exists() {
        std::fs::remove_file(&path).unwrap();
    }
    store
        .project_read_model(|projection| {
            crate::app_server::journal_transcript_messages(projection, None)
        })
        .unwrap();
    match damage {
        "missing" => std::fs::remove_file(&path).unwrap(),
        "malformed" => std::fs::write(&path, b"not a SQLite database").unwrap(),
        "checksum" => {
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute(
                "UPDATE records SET payload=x'00' WHERE section='transcript'",
                [],
            )
            .unwrap();
        }
        "unwritable" => {
            // Deterministic even when tests run as root: rename cannot replace
            // this directory. Canonical reads must not depend on cache writes.
            std::fs::remove_file(&path).unwrap();
            std::fs::create_dir(&path).unwrap();
        }
        _ => unreachable!(),
    }
}
