//! Admission remains responsive while strict cold execution state is prepared.
use super::*;
use crate::{
    core::SessionStore,
    domain::{Event, new_session_id, new_thread_id},
    model_standard::{CanonicalMessage, MessageRole},
};

async fn resumed_server(root: &std::path::Path, cwd: &std::path::Path) -> AppServerHandle {
    let store = SessionStore::new(root, cwd, new_session_id()).unwrap();
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(MessageRole::User, "saved history")],
        )
        .await
        .unwrap();
    let dir = store.session_dir().to_owned();
    drop(store);
    crate::app_server::AgentAppServer::launch_resumed(
        finalization_config(),
        cwd.into(),
        Some(&root.join("config.toml")),
        dir,
    )
    .await
    .unwrap()
}

async fn start(
    server: &AppServerHandle,
    id: &str,
    text: &str,
) -> oneshot::Receiver<Result<AgentOutput>> {
    let dispatch = bounded(server.dispatch_user_message(
        Some(id.into()),
        text.to_owned(),
        Default::default(),
        CancellationToken::new(),
    ))
    .await
    .unwrap();
    let SendDispatch::Started(result) = dispatch else {
        panic!("idle admission must start")
    };
    result
}

#[tokio::test]
async fn failed_cold_preparation_is_accepted_then_errors_without_writes_and_releases_queue() {
    for corruption in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let server = resumed_server(root.path(), cwd.path()).await;
        let store = server.runtime.session_store().unwrap();
        let original = std::fs::read(store.journal_path()).unwrap();
        let lock = if corruption {
            std::fs::write(store.journal_path(), b"{broken complete record}\n").unwrap();
            None
        } else {
            let dir = store.session_dir();
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(dir.parent().unwrap().join(format!(
                    ".{}.write.lock",
                    dir.file_name().unwrap().to_str().unwrap()
                )))
                .unwrap();
            lock.try_lock()
                .expect("display resume must not acquire the writer lease");
            Some(lock)
        };
        let before = std::fs::read(store.journal_path()).unwrap();
        let gate = server.runtime.lock_resume_validation_for_test().await;
        let mut events = server.subscribe();
        let result = start(&server, "blocked", "accepted before preparation").await;
        let queued = bounded(server.dispatch_user_message(
            Some("queued".into()),
            "queued behind preparation".to_owned(),
            Default::default(),
            CancellationToken::new(),
        ))
        .await
        .unwrap();
        let SendDispatch::Queued(queued) = queued else {
            panic!("preparing root must queue")
        };
        bounded(server.edit_queued_user_message(queued.message_id, "edited queued input".into()))
            .await
            .unwrap();
        let removed = bounded(server.dispatch_user_message(
            Some("removed".into()),
            "removed queued input".to_owned(),
            Default::default(),
            CancellationToken::new(),
        ))
        .await
        .unwrap();
        let SendDispatch::Queued(removed) = removed else {
            panic!("preparing root must queue")
        };
        bounded(server.delete_queued_user_message(removed.message_id))
            .await
            .unwrap();
        let tail = bounded(server.dispatch_user_message(
            Some("tail".into()),
            "last queued input".to_owned(),
            Default::default(),
            CancellationToken::new(),
        ))
        .await
        .unwrap();
        let SendDispatch::Queued(tail) = tail else {
            panic!("preparing root must queue")
        };
        assert_eq!(
            server
                .events
                .session_snapshot()
                .unwrap()
                .transcript
                .iter()
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>(),
            ["saved history", "accepted before preparation"]
        );
        drop(gate);
        assert!(bounded(result).await.unwrap().is_err());
        assert_eq!(std::fs::read(store.journal_path()).unwrap(), before);
        let snapshot = server.events.session_snapshot().unwrap();
        assert!(snapshot.execution.active.is_none());
        assert_eq!(snapshot.execution.last.unwrap().status, AppRunStatus::Error);
        assert_eq!(
            snapshot
                .transcript
                .iter()
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>(),
            [
                "saved history",
                "accepted before preparation",
                "edited queued input",
                "last queued input"
            ],
            "all remaining accepted inputs must remain visible exactly once in queue order"
        );
        assert_eq!(snapshot.transcript[2].message_id, Some(queued.message_id));
        assert_eq!(snapshot.transcript[3].message_id, Some(tail.message_id));
        assert!(server.runtime.queued_user_messages().await.is_empty());
        assert!(
            server
                .events
                .pending_snapshot()
                .queued_user_messages
                .is_empty()
        );
        assert!(
            bounded(server.edit_queued_user_message(queued.message_id, "late edit".into()))
                .await
                .is_err()
        );
        assert!(
            bounded(server.delete_queued_user_message(tail.message_id))
                .await
                .is_err()
        );
        let mut saw_echo = false;
        let mut saw_error = false;
        while let Ok(event) = events.try_recv() {
            match event {
                AppServerEvent::UserMessageSubmitted { .. } => saw_echo = true,
                AppServerEvent::Error { .. } => {
                    assert!(saw_echo);
                    saw_error = true;
                }
                AppServerEvent::Runtime { envelope } => assert!(!matches!(
                    envelope.event,
                    Event::TurnStarted { .. } | Event::TaskReceived { .. }
                )),
                _ => {}
            }
        }
        assert!(saw_error);
        drop(lock);
        std::fs::write(store.journal_path(), original).unwrap();
        bounded(start(&server, "repaired", "works after repair").await)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            server
                .events
                .session_snapshot()
                .unwrap()
                .transcript
                .iter()
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>(),
            ["saved history", "works after repair", "completed"]
        );
        server.shutdown().await;
    }
}

#[tokio::test]
async fn cancel_remains_responsive_during_blocked_cold_preparation() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let server = resumed_server(root.path(), cwd.path()).await;
    let store = server.runtime.session_store().unwrap();
    let before = std::fs::read(store.journal_path()).unwrap();
    let gate = server.runtime.lock_resume_validation_for_test().await;
    let result = start(&server, "preparing", "cancel before execution").await;
    assert!(matches!(
        bounded(server.dispatch_user_message(
            Some("queued-before-cancel".into()),
            "queued before cancel".to_owned(),
            Default::default(),
            CancellationToken::new(),
        ))
        .await
        .unwrap(),
        SendDispatch::Queued(_)
    ));
    bounded(server.cancel_run("preparing")).await.unwrap();
    assert_eq!(
        server.runs.lock().await.snapshot().active.unwrap().status,
        AppRunStatus::CancelRequested
    );
    drop(gate);
    assert!(bounded(result).await.unwrap().is_err());
    assert_eq!(std::fs::read(store.journal_path()).unwrap(), before);
    assert_eq!(
        server.runs.lock().await.snapshot().last.unwrap().status,
        AppRunStatus::Canceled
    );
    assert_eq!(
        server
            .events
            .session_snapshot()
            .unwrap()
            .transcript
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>(),
        [
            "saved history",
            "cancel before execution",
            "queued before cancel"
        ]
    );
    bounded(start(&server, "after-cancel", "works after cancel").await)
        .await
        .unwrap()
        .unwrap();
    server.shutdown().await;
}
