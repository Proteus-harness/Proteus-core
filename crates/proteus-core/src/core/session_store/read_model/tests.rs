#![cfg(unix)]

use std::{cell::Cell, fs, path::Path, sync::Arc};

use super::*;
use crate::{
    app_server::journal_transcript_messages,
    contracts::{ExecutionAttribution, ModelCallOrigin, OperationOwner},
    core::{
        JournalEntry, ModelRequestRecorded, ModelResponseOutcome, ModelResponseRecorded,
        PreparedSession, TurnOpened, TurnSettled, TurnSettlementStatus,
        session_journal::JournalWriterState,
    },
    domain::{
        AgentTask, ModelRef, new_exchange_id, new_execution_id, new_session_id, new_thread_id,
        new_turn_id,
    },
    model_standard::{
        CanonicalMessage, CanonicalModelRequest, CanonicalModelResponse, FinishReason, MessageRole,
        TokenUsage,
    },
};

fn render(projection: &JournalProjection) -> Vec<AppTranscriptMessage> {
    journal_transcript_messages(projection, None)
}

async fn saved(root: &Path, workspace: &Path, text: &str) -> SessionStore {
    let store = SessionStore::new(root, workspace, new_session_id()).unwrap();
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(MessageRole::User, text)],
        )
        .await
        .unwrap();
    store
}

async fn add_exchange(store: &SessionStore, workspace: &Path) {
    let thread_id = new_thread_id();
    let turn_id = new_turn_id();
    let attribution =
        ExecutionAttribution::for_turn(new_execution_id(), store.session_id(), thread_id, turn_id);
    store
        .append_execution_journal_entry(
            attribution,
            JournalEntry::TurnOpened(TurnOpened {
                intent: None,
                task: AgentTask::new("synthetic view cache", workspace.into()),
                base_history_revision: store.load_projection().unwrap().history_revision,
                module_epoch: 0,
                config_snapshot: serde_json::json!({}),
            }),
        )
        .await
        .unwrap();
    let exchange_id = new_exchange_id();
    store
        .append_execution_journal_entry(
            attribution,
            JournalEntry::ModelRequestRecorded(ModelRequestRecorded {
                exchange_id,
                origin: ModelCallOrigin::Direct,
                owner: OperationOwner::Workflow,
                request: CanonicalModelRequest::new(
                    ModelRef::new("fixture", "view-cache"),
                    vec![CanonicalMessage::text(
                        MessageRole::User,
                        "synthetic request",
                    )],
                ),
            }),
        )
        .await
        .unwrap();
    let answer = CanonicalMessage::text(MessageRole::Assistant, "synthetic answer");
    store
        .append_execution_journal_entry(
            attribution,
            JournalEntry::ModelResponseRecorded(ModelResponseRecorded {
                exchange_id,
                outcome: ModelResponseOutcome::Response {
                    response: CanonicalModelResponse::new(
                        answer.clone(),
                        vec![],
                        FinishReason::Stop,
                    )
                    .with_usage(TokenUsage::new(123, 45)),
                },
            }),
        )
        .await
        .unwrap();
    store
        .append_history(thread_id, Some(turn_id), &[answer])
        .await
        .unwrap();
    store
        .append_journal_entry(
            thread_id,
            Some(turn_id),
            JournalEntry::TurnSettled(TurnSettled {
                status: TurnSettlementStatus::Success,
                output: None,
                error: None,
            }),
        )
        .await
        .unwrap();
}

fn assert_same_view(actual: &SessionReadModel, expected: &SessionReadModel) {
    assert_eq!(actual.conversation, expected.conversation);
    assert_eq!(actual.usage, expected.usage);
    assert_eq!(actual.summary, expected.summary);
    assert_eq!(
        serde_json::to_value(&actual.transcript).unwrap(),
        serde_json::to_value(&expected.transcript).unwrap()
    );
}

#[tokio::test]
async fn sqlite_pages_keep_revision_order_and_usage_does_not_decode_conversation() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path(), workspace.path(), new_session_id()).unwrap();
    let messages: Vec<_> = (0..121)
        .map(|index| CanonicalMessage::text(MessageRole::User, format!("message {index}")))
        .collect();
    store
        .append_history(new_thread_id(), None, &messages)
        .await
        .unwrap();
    let journal = fs::read(store.journal_path()).unwrap();
    let model = store.project_read_model(render).unwrap();
    let tail = store.saved_transcript_page(None, None).unwrap();
    assert_eq!((tail.start, tail.total, tail.messages.len()), (71, 121, 50));
    let middle = store
        .saved_transcript_page(Some(tail.start), Some(&tail.revision))
        .unwrap();
    let first = store
        .saved_transcript_page(Some(middle.start), Some(&tail.revision))
        .unwrap();
    assert_eq!((middle.start, first.start), (21, 0));
    let all: Vec<_> = first
        .messages
        .into_iter()
        .chain(middle.messages)
        .chain(tail.messages)
        .map(|message| message.text)
        .collect();
    assert_eq!(
        all,
        model
            .transcript
            .iter()
            .map(|message| message.text.clone())
            .collect::<Vec<_>>()
    );
    assert!(
        store
            .saved_transcript_page(Some(122), Some(&tail.revision))
            .is_none()
    );
    assert!(
        store
            .saved_transcript_page(None, Some("wrong revision"))
            .is_none()
    );
    let db = rusqlite::Connection::open(store.session_dir().join(".read-view.sqlite")).unwrap();
    db.execute(
        "UPDATE records SET payload=x'00' WHERE section='conversation'",
        [],
    )
    .unwrap();
    drop(db);
    assert!(store.fresh_read_model().is_none());
    assert_eq!(store.fresh_read_usage().unwrap(), model.usage);
    assert_eq!(
        serde_json::to_value(store.fresh_context_summary().unwrap()).unwrap(),
        serde_json::to_value(crate::core::summarize_context_history(&messages)).unwrap()
    );
    assert!(store.saved_transcript_page(None, None).is_some());
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(MessageRole::User, "new")],
        )
        .await
        .unwrap();
    assert!(store.saved_transcript_page(None, None).is_none());
    assert!(
        store
            .saved_transcript_page(Some(71), Some(&tail.revision))
            .is_some()
    );
    assert!(store.fresh_read_usage().is_none());
    assert_ne!(fs::read(store.journal_path()).unwrap(), journal);
    store.project_read_model(render).unwrap();
    let current = store.saved_transcript_page(None, None).unwrap();
    let base = current.base.unwrap();
    assert_eq!(base.revision, tail.revision);
    assert_eq!(base.common_prefix, 121);
    assert!(
        store
            .saved_transcript_page(Some(71), Some(&tail.revision))
            .is_none()
    );
}

#[tokio::test]
async fn persisted_view_retains_transcript_conversation_usage_without_reprojection() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "synthetic user").await;
    add_exchange(&store, workspace.path()).await;
    let journal = fs::read(store.journal_path()).unwrap();
    let expected = store.project_read_model(render).unwrap();
    assert_eq!(expected.conversation.messages.len(), 2);
    assert!(!expected.transcript.is_empty());
    assert_eq!(expected.usage.requests.len(), 1);
    assert_eq!(
        expected.usage.requests[0].usage.as_ref().unwrap(),
        &TokenUsage::new(123, 45)
    );
    let (mut cached_store, actual, deferred, stale) =
        PreparedSession::from_dir(store.session_dir().to_owned())
            .unwrap()
            .into_read_model(|_| panic!("fresh persisted view must not reproject transcript"))
            .unwrap();
    assert!(deferred);
    assert!(!stale);
    assert_same_view(&actual, &expected);
    // Simulate a new process whose writer has no in-memory journal projection.
    cached_store.writer = Arc::new(tokio::sync::Mutex::new(JournalWriterState::default()));
    assert_eq!(
        cached_store.history_revision().await.unwrap(),
        expected.conversation.revision
    );
    assert_eq!(cached_store.usage_snapshot().await.unwrap(), expected.usage);
    assert_eq!(fs::read(store.journal_path()).unwrap(), journal);
}

#[tokio::test]
async fn external_append_shows_old_view_then_explicit_refresh_publishes_current_view() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "first").await;
    let old = store.project_read_model(render).unwrap();
    let external = SessionStore::open(store.session_dir().to_owned()).unwrap();
    external
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(
                MessageRole::Assistant,
                "external reply",
            )],
        )
        .await
        .unwrap();
    let journal = fs::read(store.journal_path()).unwrap();
    let (prepared_store, stale_model, deferred, stale) =
        PreparedSession::from_dir(store.session_dir().to_owned())
            .unwrap()
            .into_read_model(|_| panic!("stale display must be immediate without rendering"))
            .unwrap();
    assert!(deferred && stale);
    assert_same_view(&stale_model, &old);
    let refreshed = prepared_store.project_read_model(render).unwrap();
    assert_eq!(refreshed.conversation.messages.len(), 2);
    assert!(refreshed.conversation.revision > old.conversation.revision);
    let (_, current, deferred, stale) = PreparedSession::from_dir(store.session_dir().to_owned())
        .unwrap()
        .into_read_model(|_| panic!("strict refresh must persist its new view"))
        .unwrap();
    assert!(deferred && !stale);
    assert_same_view(&current, &refreshed);
    assert_eq!(fs::read(store.journal_path()).unwrap(), journal);
}

#[tokio::test]
async fn malformed_or_checksum_invalid_view_falls_back_to_strict_rendering() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "canonical").await;
    let journal = fs::read(store.journal_path()).unwrap();
    let path = store.session_dir().join(".read-view.sqlite");
    for damage in ["malformed", "checksum"] {
        let expected = store.project_read_model(render).unwrap();
        if damage == "malformed" {
            fs::write(&path, b"{broken").unwrap();
        } else {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute(
                    "UPDATE records SET payload=x'30' WHERE section='conversation'",
                    [],
                )
                .unwrap();
        }
        let rendered = Cell::new(false);
        let (_, actual, deferred, stale) =
            PreparedSession::from_dir(store.session_dir().to_owned())
                .unwrap()
                .into_read_model(|projection| {
                    rendered.set(true);
                    render(projection)
                })
                .unwrap();
        assert!(rendered.get(), "{damage}");
        assert!(!deferred && !stale);
        assert_same_view(&actual, &expected);
        assert_eq!(fs::read(store.journal_path()).unwrap(), journal);
    }
}

#[tokio::test]
async fn corrupted_blob_allows_stale_display_but_strict_refresh_fails_without_source_writes() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(
        root.path(),
        workspace.path(),
        &"synthetic large ".repeat(25_000),
    )
    .await;
    let old = store.project_read_model(render).unwrap();
    let journal = fs::read(store.journal_path()).unwrap();
    let blob = fs::read_dir(store.session_dir().join("blobs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut damaged = fs::read(&blob).unwrap();
    damaged[0] ^= 1;
    fs::write(&blob, &damaged).unwrap();
    let (cached_store, display, deferred, stale) =
        PreparedSession::from_dir(store.session_dir().to_owned())
            .unwrap()
            .into_read_model(|_| panic!("damaged source must not block cached display"))
            .unwrap();
    assert!(deferred && stale);
    assert_same_view(&display, &old);
    assert!(
        cached_store
            .project_read_model(|_| panic!("invalid source must fail before rendering"))
            .is_err()
    );
    assert_eq!(fs::read(store.journal_path()).unwrap(), journal);
    assert_eq!(fs::read(&blob).unwrap(), damaged);
}
