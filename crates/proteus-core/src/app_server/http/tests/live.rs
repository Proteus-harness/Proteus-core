use super::*;
use proteus_contracts::app_protocol::AppRunStatus;
use std::{future::Future, task::Poll};

#[tokio::test]
async fn deleting_session_survives_waiter_cancel_without_blocking_other_sessions() {
    let (state, server, _config_dir) = test_state().await;
    server.start_session().await.expect("start A");
    let session_a = server.session_dir_path().expect("session A");
    let cancellation = CancellationToken::new();
    let (settled, settlement) = tokio::sync::watch::channel(false);
    server
        .register_test_run_until("held-settlement", cancellation.clone(), settlement)
        .await;

    let deleting = {
        let state = state.clone();
        let session_a = session_a.clone();
        tokio::spawn(async move { execute_delete_session(&state, None, session_a).await })
    };
    tokio::time::timeout(Duration::from_secs(5), cancellation.cancelled())
        .await
        .expect("delete reached settlement");
    assert!(!deleting.is_finished(), "delete must await its own run");
    // Disconnecting the requester does not cancel the admitted deletion. Its
    // owner must keep both the closing runtime and path gate until completion.
    deleting.abort();
    assert!(
        deleting
            .await
            .expect_err("request waiter aborted")
            .is_cancelled()
    );

    // The journal spelling and directory spelling must share one gate, even
    // while this unsaved session has no physical directory yet.
    let mut resuming = Box::pin(execute_resume(
        &state,
        None,
        session_a.join("journal.jsonl"),
    ));
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(resuming.as_mut().poll(cx)))
            .await
            .is_pending(),
        "resume cannot return the retiring generation"
    );

    let unrelated = async {
        let opened = execute_new_session(&state, None, None, None).await;
        let StdioOutput::Response {
            ok: true,
            output: Some(opened),
            ..
        } = opened
        else {
            panic!("new B failed: {opened:?}");
        };
        let session_b: proteus_contracts::app_protocol::http::SessionOpened =
            serde_json::from_value(opened).expect("opened B");
        assert_ne!(session_b.session_dir, session_a);
        assert!(matches!(
            execute_resume(&state, None, session_b.session_dir.clone()).await,
            StdioOutput::Response { ok: true, .. }
        ));
        assert!(matches!(
            execute_delete_session(&state, None, session_b.session_dir.clone()).await,
            StdioOutput::Response { ok: true, .. }
        ));
        assert!(
            state
                .server_for_session_dir(&session_b.session_dir)
                .await
                .is_none()
        );
    };
    tokio::time::timeout(Duration::from_secs(5), unrelated)
        .await
        .expect("other lifecycle operations must not wait for A");
    assert!(state.server_for_session_dir(&session_a).await.is_some());

    settled.send_replace(true);
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), resuming)
            .await
            .expect("resume waits for owned deletion to complete"),
        StdioOutput::Response { ok: false, .. }
    ));
    assert!(state.server_for_session_dir(&session_a).await.is_none());
    assert!(
        !session_a.exists(),
        "resume must not recreate deleted storage"
    );
}

#[tokio::test]
async fn delete_waits_for_settlement_and_closes_admission_on_existing_handles() {
    let (state, server, _config_dir) = dogfood_loop_state().await;
    let mut events = server.subscribe();
    let output = execute_send_async(
        &state,
        Some("delete-run".into()),
        "apply_patch".into(),
        Default::default(),
        server.session_dir_path().unwrap(),
    )
    .await;
    assert!(matches!(output, StdioOutput::Response { ok: true, .. }));
    wait_for_approval_request(&mut events).await;
    let before_mode = server.permission_mode().await;
    let rejected = execute_send_async(
        &state,
        Some("queued-options".into()),
        "planning must not become steering".into(),
        crate::domain::RunOptions {
            intent: Some("planning.start".into()),
            permission_mode: Some(PermissionMode::Plan),
        },
        server.session_dir_path().unwrap(),
    )
    .await;
    assert!(matches!(rejected, StdioOutput::Response { ok: false, .. }));
    assert!(
        server
            .pending_requests()
            .await
            .queued_user_messages
            .is_empty()
    );
    assert_eq!(server.permission_mode().await, before_mode);

    server
        .clear_history()
        .await
        .expect_err("cannot clear an active run");
    let session_dir = server.session_dir_path().unwrap();
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        route_request(
            state.clone(),
            authed_json_request("/delete-session", json!({"session_dir":session_dir})),
        ),
    )
    .await
    .expect("delete settles")
    .unwrap();
    assert!(matches!(
        response_output(response).await,
        StdioOutput::Response { ok: true, .. }
    ));
    assert!(server.running_run_ids().await.is_empty());
    let terminal = loop {
        if let AppServerEvent::ExecutionUpdated { execution } =
            events.try_recv().expect("terminal published before delete")
        {
            if let Some(last) = execution.last {
                break last;
            }
        }
    };
    assert_eq!(terminal.status, AppRunStatus::Canceled);
    assert!(
        !session_dir.exists(),
        "all settlement writes completed before storage deletion"
    );
    assert!(state.server_for_session_dir(&session_dir).await.is_none());
    assert!(
        server
            .dispatch_user_message(
                Some("late".into()),
                "must not run".to_owned(),
                Default::default(),
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert!(
        !session_dir.exists(),
        "a cached handle cannot recreate deleted storage"
    );
}
