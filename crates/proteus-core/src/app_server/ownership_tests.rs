use super::*;
use crate::model_standard::{CanonicalMessage, MessageRole};

#[tokio::test]
async fn direct_owned_delete_closes_admission_and_retires_retained_store() {
    let config = tempfile::tempdir().expect("config");
    let workspace = tempfile::tempdir().expect("workspace");
    let server = AgentAppServer::launch(
        crate::test_model::config(),
        workspace.path().to_owned(),
        Some(&config.path().join("config.toml")),
    )
    .await
    .expect("server");
    let store = server.runtime.session_store().expect("store").clone();
    let thread = new_thread_id();
    let message = CanonicalMessage::text(MessageRole::User, "direct delete");
    store
        .append_history(thread, None, &[message.clone()])
        .await
        .expect("materialize");
    let retained = server.clone();
    let directory = store.session_dir().to_owned();
    assert!(
        server
            .delete_workspace_session(directory.clone())
            .await
            .expect("owned delete")
    );
    assert!(!directory.exists());
    let error = store
        .append_history(thread, None, &[message])
        .await
        .expect_err("OWNERSHIP_DIRECT_DELETE: retained store is retired");
    assert!(
        error.to_string().contains("retired"),
        "OWNERSHIP_DIRECT_DELETE: expected retired generation, got {error:#}"
    );
    let error = retained
        .send_user_message("late request".into())
        .await
        .expect_err("direct deletion closes admission on every clone");
    assert!(
        error.to_string().contains("session is shutting down"),
        "{error:#}"
    );
    assert!(
        !directory.exists(),
        "OWNERSHIP_DIRECT_DELETE: no late recreation"
    );
}
