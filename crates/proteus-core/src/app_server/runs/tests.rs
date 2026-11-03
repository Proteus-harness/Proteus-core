use super::*;

#[tokio::test]
async fn rotation_reserves_only_idle_registry_and_closes_future_admission() {
    let root = tempfile::tempdir().unwrap();
    let server = crate::app_server::AgentAppServer::launch(
        crate::test_model::config(),
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
