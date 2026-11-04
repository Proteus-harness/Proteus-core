use super::*;
use std::time::Duration;

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("operation must stay responsive")
}

fn finalization_config() -> crate::core::AppConfig {
    let mut config = crate::core::AppConfig::default();
    config.active_provider = None;
    config.providers.clear();
    config.modules.workflow = Some("finalization_probe".into());
    config.components.insert("finalization".into(), serde_json::from_value(serde_json::json!({
        "command":"python3", "args":[std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/finalization_workflow.py")],
        "exports":{"workflow":{"finalization_probe":{}}}
    })).unwrap());
    config
}

#[tokio::test]
async fn finalization_defers_send_without_blocking_completion_or_cancel() {
    for (previous, cancelled, successful) in [
        ("previous", false, true),
        ("fail", false, false),
        ("previous", true, false),
    ] {
        let root = tempfile::tempdir().unwrap();
        let server = crate::app_server::AgentAppServer::launch(
            finalization_config(),
            root.path().into(),
            None,
        )
        .await
        .unwrap();
        let reserved = match server
            .reserve_user_message(previous.to_owned(), Default::default())
            .await
            .unwrap()
        {
            UserMessageReservation::Start(reserved) => reserved,
            _ => panic!("first message must start"),
        };
        let cancellation = CancellationToken::new();
        if cancelled {
            cancellation.cancel();
        }
        let completion = server
            .runtime
            .run_reserved_completion(reserved, cancellation.clone())
            .await
            .unwrap();
        assert_eq!(completion.output().is_some(), successful);
        server.register_test_run("previous", cancellation).await;
        let mut events = server.subscribe();
        let next_server = server.clone();
        let next = tokio::spawn(async move {
            next_server
                .dispatch_user_message(
                    Some("next".into()),
                    "next".to_owned(),
                    Default::default(),
                    CancellationToken::new(),
                )
                .await
        });
        bounded(server.runtime.wait_deferred_admission_for_test()).await;
        // The previous completion owns the gate. Cancel still gets the registry;
        // the former `runs -> gate` inversion made this request wait forever.
        bounded(server.cancel_run("previous")).await.unwrap();
        {
            let mut runs = bounded(server.runs.lock()).await;
            assert_eq!(runs.active.as_ref().unwrap().info.run_id, "previous");
            let result = server.publish_turn_completion(Ok(completion));
            assert_eq!(result.is_ok(), successful);
            runs.active = None;
            runs.publish(&server);
        }
        let SendDispatch::Started(result) = bounded(next).await.unwrap().unwrap() else {
            panic!("send after finalization must start a new run")
        };
        bounded(result).await.unwrap().unwrap();
        let mut terminal_before_echo = false;
        let mut seen_echo = false;
        while let Ok(event) = events.try_recv() {
            match event {
                AppServerEvent::TurnOutput { .. } | AppServerEvent::Error { .. } => {
                    terminal_before_echo = true
                }
                AppServerEvent::UserMessageSubmitted { text, .. } if text == "next" => {
                    assert!(
                        terminal_before_echo,
                        "new admission overtook previous terminal output"
                    );
                    seen_echo = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(seen_echo, "second message must publish its admission");
        server.shutdown().await;
    }
}

#[tokio::test]
async fn shutdown_closes_admission_while_send_waits_for_finalization() {
    let root = tempfile::tempdir().unwrap();
    let server =
        crate::app_server::AgentAppServer::launch(finalization_config(), root.path().into(), None)
            .await
            .unwrap();
    let reserved = match server
        .reserve_user_message("previous".to_owned(), Default::default())
        .await
        .unwrap()
    {
        UserMessageReservation::Start(reserved) => reserved,
        _ => panic!("first message must start"),
    };
    let completion = server
        .runtime
        .run_reserved_completion(reserved, CancellationToken::new())
        .await
        .unwrap();
    let (settled_tx, settled) = watch::channel(false);
    server
        .register_test_run_until("previous", CancellationToken::new(), settled)
        .await;
    let next_server = server.clone();
    let next = tokio::spawn(async move {
        next_server
            .dispatch_user_message(
                Some("late".into()),
                "late".to_owned(),
                Default::default(),
                CancellationToken::new(),
            )
            .await
    });
    bounded(server.runtime.wait_deferred_admission_for_test()).await;
    let close_server = server.clone();
    let close = tokio::spawn(async move { close_server.close_runs().await });
    // Wait on the cancellation notification, not a timing sleep, before allowing
    // the already completed turn to publish its final state.
    let cancellation = {
        let runs = bounded(server.runs.lock()).await;
        runs.active.as_ref().unwrap().cancellation.clone()
    };
    bounded(cancellation.cancelled()).await;
    {
        let mut runs = bounded(server.runs.lock()).await;
        server.publish_turn_completion(Ok(completion)).unwrap();
        runs.active = None;
        runs.publish(&server);
    }
    settled_tx.send_replace(true);
    bounded(close).await.unwrap();
    assert!(
        matches!(bounded(next).await.unwrap(), Err(error) if error.to_string().contains("shutting down"))
    );
    server.shutdown().await;
}

#[tokio::test]
async fn rotation_reserves_only_idle_registry_and_closes_future_admission() {
    let root = tempfile::tempdir().unwrap();
    let server = crate::app_server::AgentAppServer::launch(
        finalization_config(),
        root.path().into(),
        Some(&root.path().join("config.toml")),
    )
    .await
    .unwrap();
    server
        .register_test_run("active", CancellationToken::new())
        .await;
    assert!(server.close_idle_runs().await.is_err());
    {
        let mut runs = server.runs.lock().await;
        assert!(
            !runs.closed,
            "rejected rotation must not alter active session"
        );
        runs.active = None;
        runs.command_active = true;
    }
    assert!(server.close_idle_runs().await.is_err());
    server.runs.lock().await.command_active = false;
    server.close_idle_runs().await.unwrap();
    assert!(
        server
            .send_user_message("late admission".into())
            .await
            .is_err()
    );
    assert!(server.close_idle_runs().await.is_err());
    server.shutdown().await;
}
