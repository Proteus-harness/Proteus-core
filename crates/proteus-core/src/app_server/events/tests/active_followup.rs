//! Reopening an active queued turn must include its settled predecessor.
use std::{future::Future, sync::Arc, time::Duration};

use anyhow::Result;
use tokio::sync::Notify;

use crate::{
    app_server::{AgentAppServer, AppServerEvent, AppServerHandle, runs::SendDispatch},
    contracts::{CancellationToken, Workflow, WorkflowOutput},
    core::{AppConfig, SessionStore, UserMessageReservation},
    domain::{AgentOutput, AgentTask, Event, EventContext},
    model_standard::{CanonicalMessage, MessageRole},
};
use proteus_contracts::app_protocol::{AppRunStatus, AppSessionSnapshot};

#[derive(Default)]
struct FollowupWorkflow {
    first_started: Notify,
    release_first: Notify,
    second_started: Notify,
    release_second: Notify,
}

#[async_trait::async_trait]
impl Workflow for FollowupWorkflow {
    async fn run(
        &self,
        task: AgentTask,
        history: Vec<CanonicalMessage>,
        ctx: crate::contracts::WorkflowInvocationContext,
    ) -> Result<WorkflowOutput> {
        let ctx = ctx.into_agent()?;
        assert_eq!(history.last().unwrap().role, MessageRole::User);
        let (partial, answer, started, release) = match task.text.as_str() {
            "Первый запрос" => (
                "Первый ответ",
                "Первый ответ",
                &self.first_started,
                &self.release_first,
            ),
            "ау" => (
                "Частичный ответ",
                "Частичный ответ завершён",
                &self.second_started,
                &self.release_second,
            ),
            _ => panic!("unexpected synthetic task"),
        };
        let message = CanonicalMessage::text(MessageRole::Assistant, answer);
        ctx.events
            .emit(
                EventContext::new(ctx.session_id, ctx.thread_id, Some(ctx.turn_id)),
                Event::AssistantTextDelta {
                    message_id: message.id,
                    phase: None,
                    text: partial.into(),
                    offset: 0,
                },
            )
            .await?;
        started.notify_one();
        release.notified().await;
        Ok(WorkflowOutput::new(
            AgentOutput::text(answer),
            vec![message],
        ))
    }
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("synthetic workflow must reach its next barrier")
}

async fn launch(
    root: &std::path::Path,
    persisted: bool,
) -> (AppServerHandle, Arc<FollowupWorkflow>) {
    let workflow = Arc::new(FollowupWorkflow::default());
    let mut config = AppConfig::default();
    config.active_provider = None;
    config.providers.clear();
    crate::test_support::select_test_modules(&mut config, "coding.single_loop");
    let mut catalog = crate::test_support::module_catalog_for(&config);
    catalog.register_test_workflow("coding.single_loop", workflow.clone());
    let config_path = root.join("configs/config.toml");
    let server = AgentAppServer::launch_with_module_catalog(
        config,
        root.to_owned(),
        persisted.then_some(config_path.as_path()),
        catalog,
    )
    .await
    .unwrap();
    assert_eq!(server.session_dir_path().is_some(), persisted);
    (server, workflow)
}

async fn reopen_snapshot(server: &AppServerHandle) -> AppSessionSnapshot {
    let mut reopened = server.subscribe_session();
    assert!(matches!(
        bounded(reopened.recv()).await.unwrap(),
        AppServerEvent::PendingRequestsUpdated { .. }
    ));
    let AppServerEvent::SessionSnapshot { snapshot } = bounded(reopened.recv()).await.unwrap()
    else {
        panic!("reopened subscription must start with its authoritative snapshot")
    };
    *snapshot
}

fn rows(snapshot: &AppSessionSnapshot) -> Vec<(&str, &str, bool)> {
    snapshot
        .transcript
        .iter()
        .map(|message| {
            (
                message.role.as_str(),
                message.text.as_str(),
                message.streaming,
            )
        })
        .collect()
}

#[tokio::test]
async fn first_turn_handoff_keeps_submitted_input_before_and_after_turn_started() {
    let root = tempfile::tempdir().unwrap();
    let (server, workflow) = launch(root.path(), true).await;
    let reserved = match server
        .reserve_user_message("Первый запрос".to_owned(), Default::default())
        .await
        .unwrap()
    {
        UserMessageReservation::Start(reserved) => reserved,
        _ => panic!("first message must reserve a turn"),
    };
    let before = reopen_snapshot(&server).await;
    assert_eq!(rows(&before), [("user", "Первый запрос", false)]);
    assert_eq!(before.root_thread_id, None);
    let runtime = server.runtime.clone();
    let completion = tokio::spawn(async move {
        runtime
            .run_reserved_completion(reserved, CancellationToken::new())
            .await
    });
    bounded(workflow.first_started.notified()).await;
    let during = reopen_snapshot(&server).await;
    assert_eq!(
        rows(&during),
        [
            ("user", "Первый запрос", false),
            ("assistant", "Первый ответ", true)
        ]
    );
    assert!(during.seq > before.seq);
    assert!(during.root_thread_id.is_some());
    workflow.release_first.notify_one();
    let completion = bounded(completion).await.unwrap().unwrap();
    drop(completion);
    server.shutdown().await;
}

#[tokio::test]
async fn active_followup_reopen_preserves_completed_prefix_cursor_and_partial_once() {
    for persisted in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let (server, workflow) = launch(root.path(), persisted).await;
        let SendDispatch::Started(result) = server
            .dispatch_user_message(
                Some("followup-chain".into()),
                "Первый запрос".to_owned(),
                Default::default(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
        else {
            panic!("idle session must start the run")
        };
        bounded(workflow.first_started.notified()).await;
        let before = reopen_snapshot(&server).await;
        assert_eq!(
            rows(&before),
            [
                ("user", "Первый запрос", false),
                ("assistant", "Первый ответ", true)
            ]
        );
        assert!(matches!(
            server
                .dispatch_user_message(
                    Some("queued-input".into()),
                    "ау".to_owned(),
                    Default::default(),
                    CancellationToken::new(),
                )
                .await
                .unwrap(),
            SendDispatch::Queued(_)
        ));
        workflow.release_first.notify_one();
        bounded(workflow.second_started.notified()).await;
        let during = reopen_snapshot(&server).await;
        assert_eq!(
            rows(&during),
            [
                ("user", "Первый запрос", false),
                ("assistant", "Первый ответ", false),
                ("user", "ау", false),
                ("assistant", "Частичный ответ", true),
            ]
        );
        assert!(during.seq > before.seq);
        assert_eq!(during.root_thread_id, before.root_thread_id);
        let run = during.execution.active.as_ref().unwrap();
        assert_eq!(run.run_id, "followup-chain");
        assert_eq!(run.status, AppRunStatus::Running);
        assert!(during.execution.last.is_none());
        if persisted {
            let cursor = during
                .history
                .as_ref()
                .expect("stored prefix must retain its cursor");
            assert_eq!(cursor.before, 0);
            let store = SessionStore::open(server.session_dir_path().unwrap()).unwrap();
            let page = crate::app_server::read_model::transcript_page(
                &store,
                None,
                Some(&cursor.revision),
            )
            .unwrap();
            assert_eq!(
                page.total, 2,
                "active turn must stay outside the completed page"
            );
            assert_eq!(page.messages.len(), 2);
            assert_eq!(page.messages[0].text, "Первый запрос");
            assert_eq!(page.messages[1].text, "Первый ответ");
        } else {
            assert!(during.history.is_none());
        }
        let repeated = reopen_snapshot(&server).await;
        assert_eq!(
            serde_json::to_value(&repeated).unwrap(),
            serde_json::to_value(&during).unwrap()
        );
        workflow.release_second.notify_one();
        assert_eq!(
            bounded(result).await.unwrap().unwrap().text,
            "Частичный ответ завершён"
        );
        let after = reopen_snapshot(&server).await;
        assert_eq!(
            rows(&after),
            [
                ("user", "Первый запрос", false),
                ("assistant", "Первый ответ", false),
                ("user", "ау", false),
                ("assistant", "Частичный ответ завершён", false),
            ]
        );
        assert!(after.execution.active.is_none());
        assert_eq!(after.execution.last.unwrap().status, AppRunStatus::Success);
        if let Some(dir) = server.session_dir_path() {
            let store = SessionStore::open(dir).unwrap();
            let cold = store
                .project_read_model(|source| {
                    crate::app_server::journal_transcript_from_source(source, None)
                })
                .unwrap();
            assert_eq!(
                serde_json::to_value(cold.transcript).unwrap(),
                serde_json::to_value(after.transcript).unwrap()
            );
        }
        server.shutdown().await;
    }
}
