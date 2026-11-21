use super::*;

fn scoped_path(path: &str, server: &AppServerHandle) -> String {
    // Test temp paths contain only URI-safe ASCII bytes.
    format!(
        "{path}?session_dir={}",
        server
            .session_dir_path()
            .expect("session storage")
            .display()
    )
}

async fn read_json_at(state: &HttpAppState, path: &str) -> Value {
    let response = route_request(state.clone(), authed_get_request(path))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    serde_json::from_slice(&response_bytes(response).await).unwrap()
}

async fn post_at(state: &HttpAppState, path: &str, body: Value) -> StdioOutput {
    let response = route_request(state.clone(), authed_json_request(path, body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    response_output(response).await
}

fn command_value(output: StdioOutput) -> Value {
    match output {
        StdioOutput::Response {
            ok: true,
            output: Some(value),
            ..
        } => value,
        other => panic!("expected command result, got {other:?}"),
    }
}

async fn next_message(body: &mut HttpBody) -> String {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let frame = body.frame().await.expect("SSE open").expect("SSE frame");
            let Ok(data) = frame.into_data() else {
                continue;
            };
            for line in std::str::from_utf8(&data).unwrap().lines() {
                let Some(json) = line.strip_prefix("data: ") else {
                    continue;
                };
                let output: StdioOutput = serde_json::from_str(json).unwrap();
                if let StdioOutput::Event { event } = output {
                    match *event {
                        AppServerEvent::UserMessageSubmitted { text, .. } => return text,
                        AppServerEvent::SessionSnapshot { snapshot } => {
                            if let Some(message) =
                                snapshot.transcript.iter().rev().find(|m| m.role == "user")
                            {
                                return message.text.clone();
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    })
    .await
    .expect("addressed message arrives")
}

#[tokio::test]
async fn independent_connections_keep_config_pending_and_sse_bound_to_their_sessions() {
    let cwd = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let config_path = config_dir.path().join("config.toml");
    let a = AgentAppServer::launch(
        crate::test_model::config(),
        cwd.path().to_owned(),
        Some(&config_path),
    )
    .await
    .unwrap();
    let (shutdown, _) = broadcast::channel(1);
    let state = HttpAppState::new(a.clone(), shutdown, test_security()).await;
    let initial = read_json_at(&state, "/bootstrap").await;
    let _: proteus_contracts::app_protocol::AppBootstrap =
        serde_json::from_value(initial.clone()).unwrap();
    let created = command_value(post_at(&state, "/new-session", json!({"id":"create-b"})).await);
    let b_dir = PathBuf::from(created["session_dir"].as_str().unwrap());
    let b = state.server_for_session_dir(&b_dir).await.unwrap();

    command_value(
        post_at(
            &state,
            "/mode",
            json!({"mode":"plan", "session_dir":a.session_dir_path()}),
        )
        .await,
    );
    command_value(
        post_at(
            &state,
            "/mode",
            json!({"mode":"auto", "session_dir":b.session_dir_path()}),
        )
        .await,
    );
    assert_eq!(
        read_json_at(&state, &scoped_path("/config", &a)).await["permission_mode"],
        "Plan"
    );
    assert_eq!(
        read_json_at(&state, &scoped_path("/config", &b)).await["permission_mode"],
        "Auto"
    );

    let mut events_a = route_request(
        state.clone(),
        authed_get_request(&scoped_path("/events", &a)),
    )
    .await
    .unwrap()
    .into_body();
    let mut events_b = route_request(
        state.clone(),
        authed_get_request(&scoped_path("/events", &b)),
    )
    .await
    .unwrap()
    .into_body();
    // Start both independent HTTP responses before publishing any session events.
    events_a.frame().await.unwrap().unwrap();
    events_b.frame().await.unwrap().unwrap();
    let (approval_tx_a, approval_rx_a) = tokio::sync::oneshot::channel();
    let (approval_tx_b, _approval_rx_b) = tokio::sync::oneshot::channel();
    register_pending_approval(&a, "approval-a", approval_tx_a).await;
    register_pending_approval(&b, "approval-b", approval_tx_b).await;
    assert_eq!(
        read_json_at(&state, &scoped_path("/pending", &a)).await["approvals"][0]["approval_id"],
        "approval-a"
    );
    assert_eq!(
        read_json_at(&state, &scoped_path("/pending", &b)).await["approvals"][0]["approval_id"],
        "approval-b"
    );

    // Opening B and then A never alters another connection or the bootstrap suggestion.
    for server in [&b, &a] {
        command_value(
            post_at(
                &state,
                "/resume",
                json!({"session_dir":server.session_dir_path()}),
            )
            .await,
        );
    }
    assert_eq!(read_json_at(&state, "/bootstrap").await, initial);
    assert_eq!(state.all_servers().await.len(), 2);
    b.events
        .send(AppServerEvent::UserMessageSubmitted {
            images: Vec::new(),
            text: "B only".into(),
        })
        .unwrap();
    a.events
        .send(AppServerEvent::UserMessageSubmitted {
            images: Vec::new(),
            text: "A only".into(),
        })
        .unwrap();
    assert_eq!(next_message(&mut events_a).await, "A only");
    assert_eq!(next_message(&mut events_b).await, "B only");

    // Reconnect B while A was most recently resumed. The URL retains B.
    drop(events_b);
    let mut events_b = route_request(
        state.clone(),
        authed_get_request(&scoped_path("/events", &b)),
    )
    .await
    .unwrap()
    .into_body();
    events_b.frame().await.unwrap().unwrap();
    a.events
        .send(AppServerEvent::UserMessageSubmitted {
            images: Vec::new(),
            text: "A second".into(),
        })
        .unwrap();
    b.events
        .send(AppServerEvent::UserMessageSubmitted {
            images: Vec::new(),
            text: "B reconnected".into(),
        })
        .unwrap();
    assert_eq!(next_message(&mut events_b).await, "B reconnected");
    assert_eq!(next_message(&mut events_a).await, "A second");

    let answer = json!({"approval_id":"approval-a", "approved":true});
    assert!(matches!(
        post_at(&state, &scoped_path("/approval", &b), answer.clone()).await,
        StdioOutput::Response { ok: false, .. }
    ));
    assert!(a.has_pending_approval("approval-a").await);
    assert!(matches!(
        post_at(&state, &scoped_path("/approval", &a), answer).await,
        StdioOutput::Response { ok: true, .. }
    ));
    assert!(approval_rx_a.await.unwrap().approved);
    assert!(b.has_pending_approval("approval-b").await);

    let cancel_a = CancellationToken::new();
    let cancel_b = CancellationToken::new();
    a.register_test_run("run-a", cancel_a.clone()).await;
    b.register_test_run("run-b", cancel_b.clone()).await;
    assert!(matches!(
        post_at(
            &state,
            &scoped_path("/cancel", &b),
            json!({"target_id":"run-a"})
        )
        .await,
        StdioOutput::Response { ok: false, .. }
    ));
    assert!(!cancel_a.is_cancelled());
    assert!(matches!(
        post_at(
            &state,
            &scoped_path("/cancel", &a),
            json!({"target_id":"run-a"})
        )
        .await,
        StdioOutput::Response { ok: true, .. }
    ));
    assert!(cancel_a.is_cancelled());
    assert!(!cancel_b.is_cancelled());

    drop(events_a);
    drop(events_b);
    command_value(
        post_at(
            &state,
            "/delete-session",
            json!({"session_dir":a.session_dir_path()}),
        )
        .await,
    );
    assert_eq!(
        read_json_at(&state, "/bootstrap").await["session_dir"],
        Value::Null
    );
    assert!(state.server_for_session_dir(&b_dir).await.is_some());
    assert_eq!(
        state.all_servers().await.len(),
        1,
        "deletion does not create a replacement chat"
    );
    assert_eq!(
        read_json_at(&state, &scoped_path("/config", &b)).await["permission_mode"],
        "Auto"
    );
    b.shutdown().await;
}

#[tokio::test]
async fn session_routes_reject_missing_unknown_and_ambiguous_addresses() {
    let cwd = tempfile::tempdir().unwrap();
    let config_path = cwd.path().join("config.toml");
    let server = AgentAppServer::launch(
        crate::test_model::config(),
        cwd.path().to_owned(),
        Some(&config_path),
    )
    .await
    .unwrap();
    let (shutdown, _) = broadcast::channel(1);
    let state = HttpAppState::new(server.clone(), shutdown, test_security()).await;
    for path in [
        "/events",
        "/config",
        "/config/local",
        "/pending",
        "/model/quota",
        "/history",
        "/conversation",
        "/usage",
        "/context",
        "/sessions/current",
        "/config/builder",
        "/config/history",
        "/inspect/topology",
        "/inspect/plan",
    ] {
        for suffix in [
            "",
            "?session_dir=",
            "?session_dir=relative",
            "?session_dir=/missing-session",
            "?session_dir=/one&session_dir=/two",
        ] {
            let response = route_request(
                state.clone(),
                authed_get_request(&format!("{path}{suffix}")),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}{suffix}");
        }
    }
    for (path, body) in [
        ("/send-async", json!({"text":"must not run"})),
        ("/send", json!({"text":"must not run"})),
        ("/mode", json!({"mode":"auto"})),
        ("/model", json!({"model":"wrong"})),
        ("/effort", json!({"effort":"high"})),
        ("/reasoning", json!({"enabled":false})),
        ("/config/builder", json!({"modules":{}})),
    ] {
        let response = route_request(state.clone(), authed_json_request(path, body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
    }
    for body in [
        json!({"type":"config_summary"}),
        json!({"type":"send","text":"must not run"}),
    ] {
        assert!(matches!(
            post_at(&state, "/request", body).await,
            StdioOutput::Response { ok: false, .. }
        ));
    }
    assert_eq!(server.permission_mode().await, PermissionMode::Normal);
    assert!(server.transcript().await.unwrap().is_empty());
    assert!(server.running_run_ids().await.is_empty());
    server.shutdown().await;
}

#[tokio::test]
async fn supervised_saved_chat_skips_unrelated_live_handle_and_keeps_launch_workspace() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let config_path = root.path().join("config.toml");
    let storage = crate::test_model::storage(a.path());
    let mut dirs = Vec::new();
    for (workspace, text) in [(a.path(), "unrelated A"), (b.path(), "required B")] {
        let store = crate::core::SessionStore::new(
            storage.clone(),
            root.path(),
            workspace,
            crate::domain::new_session_id(),
        )
        .unwrap();
        store
            .append_history(
                crate::domain::new_thread_id(),
                None,
                &[crate::model_standard::CanonicalMessage::text(
                    crate::model_standard::MessageRole::User,
                    text,
                )],
            )
            .await
            .unwrap();
        dirs.push(store.session_dir().to_owned());
    }
    let missing = root.path().join("deleted");
    for (hint, initial, count) in [
        (None, &dirs[0], 2),
        (Some(dirs[1].clone()), &dirs[1], 1),
        (Some(missing), &dirs[0], 2),
    ] {
        let (server, default_workspace) = crate::app_server::startup::launch_http_selected(
            crate::test_model::config(),
            a.path().to_owned(),
            Some(&config_path),
            None,
            false,
            hint,
        )
        .await
        .unwrap();
        assert_eq!(server.session_dir_path().as_ref(), Some(initial));
        let (shutdown, _) = broadcast::channel(1);
        let state =
            HttpAppState::new_for_workspace(server, default_workspace, shutdown, test_security())
                .await;
        // Real handler path used by Desktop, including reuse for its second /resume.
        let opened =
            command_value(post_at(&state, "/resume", json!({"session_dir":dirs[1]})).await);
        assert_eq!(opened["workspace_path"], json!(b.path()));
        assert_eq!(opened["session_dir"], json!(dirs[1]));
        let live = state.server_for_session_dir(&dirs[1]).await.unwrap();
        assert_eq!(live.transcript().await.unwrap()[0].text, "required B");
        assert_eq!(state.all_servers().await.len(), count);
        let bootstrap = read_json_at(&state, "/bootstrap").await;
        assert_eq!(bootstrap["session_dir"], json!(initial));
        assert_eq!(bootstrap["cwd"], json!(a.path()));
        let new_session = command_value(post_at(&state, "/new-session", json!({})).await);
        assert_eq!(new_session["workspace_path"], json!(a.path()));
        for live in state.all_servers().await {
            live.shutdown().await;
        }
    }
}

#[tokio::test]
async fn lifecycle_uses_the_explicit_source_workspace_and_keeps_deletion_in_the_config_store() {
    let workspace_a = tempfile::tempdir().unwrap();
    let workspace_b = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let config_path = config_dir.path().join("config.toml");
    let a = AgentAppServer::launch(
        crate::test_model::config(),
        workspace_a.path().to_owned(),
        Some(&config_path),
    )
    .await
    .unwrap();
    let (shutdown, _) = broadcast::channel(1);
    let state = HttpAppState::new(a.clone(), shutdown, test_security()).await;
    let stored_b = crate::core::SessionStore::new(
        state.storage_service().unwrap(),
        config_dir.path(),
        workspace_b.path(),
        crate::domain::new_session_id(),
    )
    .unwrap();
    stored_b
        .append_history(
            crate::domain::new_thread_id(),
            None,
            &[crate::model_standard::CanonicalMessage::text(
                crate::model_standard::MessageRole::User,
                "workspace B history",
            )],
        )
        .await
        .unwrap();
    // Restore B under launch workspace A before requesting any session catalog
    // or config metadata. Resource extensions need its exact workspace now.
    for _ in 0..2 {
        let resumed = command_value(
            post_at(
                &state,
                "/resume",
                json!({"session_dir":stored_b.session_dir()}),
            )
            .await,
        );
        let opened: proteus_contracts::app_protocol::http::SessionOpened =
            serde_json::from_value(resumed).unwrap();
        assert_eq!(opened.session_dir, stored_b.session_dir());
        assert_eq!(opened.workspace_path, workspace_b.path());
    }
    command_value(
        post_at(
            &state,
            "/mode",
            json!({"session_dir":stored_b.session_dir(),"mode":"auto"}),
        )
        .await,
    );
    let created = command_value(
        post_at(
            &state,
            "/new-session",
            json!({"source_session_dir":stored_b.session_dir()}),
        )
        .await,
    );
    assert_eq!(
        created["workspace_path"],
        workspace_b.path().to_str().unwrap()
    );
    let created_dir = PathBuf::from(created["session_dir"].as_str().unwrap());
    let created_server = state.server_for_session_dir(&created_dir).await.unwrap();
    let config = read_json_at(&state, &scoped_path("/config", &created_server)).await;
    assert_eq!(config["cwd"], workspace_b.path().to_str().unwrap());
    assert_eq!(config["permission_mode"], "Auto");
    assert_eq!(
        created_server.config.read().await.permissions.mode,
        PermissionMode::Normal,
        "inherited session overrides must not rewrite the published profile"
    );
    assert!(created_dir.starts_with(config_dir.path()));
    let elsewhere = tempfile::tempdir().unwrap();
    let opened = command_value(
        post_at(
            &state,
            "/new-session",
            json!({"source_session_dir":stored_b.session_dir(),"workspace":elsewhere.path()}),
        )
        .await,
    );
    assert_eq!(
        opened["workspace_path"],
        std::fs::canonicalize(elsewhere.path())
            .unwrap()
            .to_str()
            .unwrap()
    );
    let opened_dir = PathBuf::from(opened["session_dir"].as_str().unwrap());
    let opened_server = state.server_for_session_dir(&opened_dir).await.unwrap();
    let config = read_json_at(&state, &scoped_path("/config", &opened_server)).await;
    assert_eq!(
        config["cwd"],
        std::fs::canonicalize(elsewhere.path())
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert_eq!(config["permission_mode"], "Auto");
    assert_eq!(
        opened_server.config.read().await.permissions.mode,
        PermissionMode::Normal,
        "workspace selection must preserve the published profile baseline"
    );
    assert!(
        command_value(
            post_at(
                &state,
                "/delete-session",
                json!({"session_dir":stored_b.session_dir()})
            )
            .await
        )["deleted"]
            .as_bool()
            .unwrap()
    );
    assert!(!stored_b.session_dir().exists());
    assert_eq!(
        read_json_at(&state, "/bootstrap").await["session_dir"],
        a.session_dir_path().unwrap().to_str().unwrap()
    );
    assert!(state.server_for_session_dir(&created_dir).await.is_some());

    // A valid session from another storage root may be resumed, but deletion
    // must fail before dropping its live registration or touching its files.
    let outside_store = tempfile::tempdir().unwrap();
    let outside = crate::core::SessionStore::new(
        crate::test_support::storage(std::path::Path::new(".")),
        outside_store.path(),
        workspace_b.path(),
        crate::domain::new_session_id(),
    )
    .unwrap();
    outside
        .append_history(
            crate::domain::new_thread_id(),
            None,
            &[crate::model_standard::CanonicalMessage::text(
                crate::model_standard::MessageRole::User,
                "outside storage",
            )],
        )
        .await
        .unwrap();
    command_value(
        post_at(
            &state,
            "/resume",
            json!({"session_dir":outside.session_dir()}),
        )
        .await,
    );
    assert!(matches!(
        post_at(
            &state,
            "/delete-session",
            json!({"session_dir":outside.session_dir()})
        )
        .await,
        StdioOutput::Response { ok: false, .. }
    ));
    assert!(outside.session_dir().exists());
    assert!(
        state
            .server_for_session_dir(outside.session_dir())
            .await
            .is_some()
    );
    let count = state.all_servers().await.len();
    assert!(matches!(
        post_at(
            &state,
            "/new-session",
            json!({"source_session_dir":"/missing-session"})
        )
        .await,
        StdioOutput::Response { ok: false, .. }
    ));
    assert!(matches!(
        post_at(
            &state,
            "/new-session",
            json!({"workspace":"/missing-workspace"})
        )
        .await,
        StdioOutput::Response { ok: false, .. }
    ));
    assert_eq!(state.all_servers().await.len(), count);
    for server in state.all_servers().await {
        server.shutdown().await;
    }
}

#[tokio::test]
async fn http_rejects_storage_binding_drift_before_new_or_cold_resume() {
    let (state, server, root) = test_state().await;
    let baseline = state.launch.config.clone();
    let config_path = state.launch.config_path.as_ref().unwrap();
    let initial_count = state.all_servers().await.len();
    for change in ["selection", "command", "config", "transport", "export"] {
        let mut changed = serde_json::to_value(&baseline).unwrap();
        match change {
            "selection" => changed["modules"]["session_storage"] = Value::Null,
            "command" => {
                changed["components"]["test-session-storage"]["command"] =
                    json!("must-never-launch-storage-drift")
            }
            "config" => {
                changed["module_config"]["session_storage"]["jsonl"] =
                    json!({"new_backend_setting": true})
            }
            "transport" => {
                changed["components"]["test-session-storage"]["transport"]["max_frame_bytes"] =
                    json!(64 * 1024 * 1024)
            }
            "export" => {
                changed["components"]["test-session-storage"]["exports"]["session_storage"]["jsonl"]
                    ["timeout_ms"] = json!(30_001)
            }
            _ => unreachable!(),
        }
        let changed: AppConfig = serde_json::from_value(changed).unwrap();
        std::fs::write(config_path, toml::to_string_pretty(&changed).unwrap()).unwrap();
        for response in [
            execute_new_session(&state, None, None, None).await,
            execute_resume(&state, None, root.path().join("cold-session")).await,
        ] {
            let StdioOutput::Response {
                ok: false,
                error: Some(error),
                ..
            } = response
            else {
                panic!("{change}: expected a storage binding rejection: {response:?}");
            };
            assert!(
                error.contains("restart the HTTP server"),
                "{change}: {error}"
            );
        }
        assert_eq!(state.all_servers().await.len(), initial_count, "{change}");
    }
    server.shutdown().await;
}

#[tokio::test]
async fn http_source_session_must_match_pinned_storage_but_model_settings_may_change() {
    let (state, server, _root) = test_state().await;
    server.config.write().await.runtime.context_timeout_ms += 1;
    let opened = execute_new_session(&state, None, server.session_dir_path(), None).await;
    assert!(
        matches!(opened, StdioOutput::Response { ok: true, .. }),
        "{opened:?}"
    );
    server
        .config
        .write()
        .await
        .module_config
        .entry("session_storage".into())
        .or_default()
        .insert("jsonl".into(), json!({"new_backend_setting": true}));
    let rejected = execute_new_session(&state, None, server.session_dir_path(), None).await;
    assert!(
        matches!(&rejected, StdioOutput::Response { ok: false, error: Some(error), .. }
        if error.contains("restart the HTTP server")),
        "{rejected:?}"
    );
    for server in state.all_servers().await {
        server.shutdown().await;
    }
}
