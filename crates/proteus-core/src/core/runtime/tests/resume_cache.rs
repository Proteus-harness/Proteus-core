use super::*;
use crate::{
    contracts::ConversationSnapshot,
    core::SessionStore,
    domain::{new_session_id, new_thread_id},
};

struct CaptureHistory(Arc<std::sync::Mutex<Vec<Vec<CanonicalMessage>>>>);
#[async_trait]
impl Workflow for CaptureHistory {
    async fn run(
        &self,
        task: AgentTask,
        history: Vec<CanonicalMessage>,
        _: crate::contracts::WorkflowInvocationContext,
    ) -> Result<WorkflowOutput> {
        self.0.lock().unwrap().push(history.clone());
        Ok(successful_messages(history, task, "finished"))
    }
}

async fn cold_store(root: &std::path::Path, workspace: &std::path::Path) -> SessionStore {
    let fixture = tempfile::tempdir().unwrap();
    let source = SessionStore::new(fixture.path(), workspace, new_session_id()).unwrap();
    source
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(
                MessageRole::User,
                "canonical source",
            )],
        )
        .await
        .unwrap();
    let target = root
        .join("sessions")
        .join(crate::core::encode_workspace_path(workspace).unwrap())
        .join(source.session_dir().file_name().unwrap());
    std::fs::create_dir_all(&target).unwrap();
    for file in ["session.json", "journal.jsonl"] {
        std::fs::copy(source.session_dir().join(file), target.join(file)).unwrap();
    }
    SessionStore::open(target).unwrap()
}

async fn cached_runtime(store: SessionStore, config: &std::path::Path) -> AgentRuntime {
    AgentRuntime::builder(AppConfig::default(), store.workspace_path().unwrap())
        .with_config_path(Some(config))
        .with_module_catalog(test_catalog())
        .resume_from_session_snapshot(
            store,
            ConversationSnapshot {
                revision: 900,
                messages: vec![CanonicalMessage::text(
                    MessageRole::User,
                    "incorrect cached context",
                )],
                model_context: Vec::new(),
            },
            new_thread_id(),
            true,
        )
        .build()
        .unwrap()
}

#[tokio::test]
async fn cached_context_is_reconciled_before_the_first_workflow_and_writer_lease_is_retained() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = cold_store(root.path(), workspace.path()).await;
    let runtime = cached_runtime(store.clone(), &root.path().join("config.toml")).await;
    assert_eq!(
        message_text_for_test(&runtime.history().await[0]),
        "incorrect cached context"
    );
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    replace_workflow_for_test(&runtime, Arc::new(CaptureHistory(seen.clone()))).await;
    runtime.run("continue".into()).await.unwrap();
    let calls = seen.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(message_text_for_test(&calls[0][0]), "canonical source");
    assert_eq!(message_text_for_test(calls[0].last().unwrap()), "continue");
    assert!(!*runtime.session.deferred_resume_validation.lock().await);
    assert_eq!(runtime.history().await, store.load_messages().unwrap());
}

#[tokio::test]
async fn corrupt_source_blocks_cached_continuation_before_workflow_or_canonical_writes() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = cold_store(root.path(), workspace.path()).await;
    let runtime = cached_runtime(store.clone(), &root.path().join("config.toml")).await;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    replace_workflow_for_test(&runtime, Arc::new(CaptureHistory(seen.clone()))).await;
    std::fs::write(store.journal_path(), b"{broken complete record}\n").unwrap();
    let before = std::fs::read(store.journal_path()).unwrap();
    assert!(runtime.run("must fail".into()).await.is_err());
    assert!(
        runtime
            .execute_user_command("irrelevant", "", crate::contracts::CancellationToken::new())
            .await
            .is_err()
    );
    assert!(seen.lock().unwrap().is_empty());
    assert!(*runtime.session.deferred_resume_validation.lock().await);
    assert_eq!(std::fs::read(store.journal_path()).unwrap(), before);
}
