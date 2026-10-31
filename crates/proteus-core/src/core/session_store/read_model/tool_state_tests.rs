use super::*;
use crate::{
    app_server::{journal_transcript_append_from_source, journal_transcript_from_source},
    domain::{ToolCall, ToolResult, new_session_id, new_thread_id},
    model_standard::{CanonicalMessage, ContentPart, MessageRole},
};
use serde_json::{Value, json};

fn call(id: &str, name: &str, args: Value) -> CanonicalMessage {
    CanonicalMessage::new(
        MessageRole::Assistant,
        vec![ContentPart::ToolCall {
            call: ToolCall::new(id, name, args),
        }],
    )
}

fn result(id: &str, ok: bool) -> CanonicalMessage {
    let mut result = ToolResult::ok(id.to_owned(), "output excluded from compact state");
    result.ok = ok;
    result.metadata = json!({"omitted":true});
    CanonicalMessage::new(MessageRole::Tool, vec![ContentPart::ToolResult { result }])
}

fn render(source: &mut JournalReadSnapshot) -> anyhow::Result<Vec<AppTranscriptMessage>> {
    journal_transcript_from_source(source, None)
}

async fn fixture() -> (tempfile::TempDir, tempfile::TempDir, SessionStore) {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path(), workspace.path(), new_session_id()).unwrap();
    let mut messages = vec![
        call("accepted", "opaque / exact", json!({"value":1})),
        result("accepted", true),
    ];
    messages
        .extend((0..121).map(|i| CanonicalMessage::text(MessageRole::User, format!("row {i}"))));
    messages.extend([
        call("failed", "opaque / exact", json!({"value":2})),
        result("failed", false),
    ]);
    store
        .append_history(new_thread_id(), None, &messages)
        .await
        .unwrap();
    store.project_read_model(render).unwrap();
    (root, workspace, store)
}

#[tokio::test]
async fn latest_tool_state_ignores_view_window_and_failed_rows_and_survives_reopen() {
    let (_root, _workspace, store) = fixture().await;
    let journal = std::fs::read(store.journal_path()).unwrap();
    assert_eq!(
        store
            .saved_transcript_page(None, None)
            .unwrap()
            .messages
            .len(),
        50
    );
    let state = store.saved_tool_state("opaque / exact").unwrap();
    let tool = state.tool.unwrap();
    assert_eq!(tool.call_id, "accepted");
    assert_eq!(tool.status, "done");
    assert_eq!(tool.args, json!({"value":1}));
    let wire = serde_json::to_value(&tool).unwrap();
    assert_eq!(wire.as_object().unwrap().len(), 5);
    assert!(wire.get("result").is_none() && wire.get("metadata").is_none());
    assert!(store.saved_tool_state("opaque").unwrap().tool.is_none());
    let reopened = SessionStore::open(store.session_dir().into()).unwrap();
    assert_eq!(
        reopened
            .saved_tool_state("opaque / exact")
            .unwrap()
            .tool
            .unwrap()
            .call_id,
        "accepted"
    );
    assert_eq!(std::fs::read(store.journal_path()).unwrap(), journal);
}

#[tokio::test]
async fn latest_tool_state_append_and_authoritative_empty_input_then_clear() {
    let (_root, _workspace, store) = fixture().await;
    // Establish a certified append checkpoint, then forbid a full renderer.
    store
        .project_read_view(render, journal_transcript_append_from_source)
        .unwrap();
    store
        .append_history(
            new_thread_id(),
            None,
            &[
                call("empty", "opaque / exact", Value::Null),
                result("empty", true),
            ],
        )
        .await
        .unwrap();
    assert!(
        store.saved_tool_state("opaque / exact").is_none(),
        "stale state is not returned"
    );
    store
        .project_read_view(
            |_| panic!("independent suffix must append"),
            journal_transcript_append_from_source,
        )
        .unwrap();
    let latest = store
        .saved_tool_state("opaque / exact")
        .unwrap()
        .tool
        .unwrap();
    assert_eq!(latest.call_id, "empty");
    assert!(latest.args.is_null());
    store.clear_history(new_thread_id()).await.unwrap();
    store.project_read_model(render).unwrap();
    assert!(
        store
            .saved_tool_state("opaque / exact")
            .unwrap()
            .tool
            .is_none()
    );
}

#[tokio::test]
async fn latest_tool_state_full_save_replaces_final_status_and_retains_effective_args() {
    let (_root, _workspace, store) = fixture().await;
    let mut model = store.fresh_read_model().unwrap();
    let source =
        super::super::read_cache::SourceFingerprint::read(store.session_dir(), 8192).unwrap();
    // Exercise same-position row updates independently of lifecycle status spelling.
    let last = model.transcript.last_mut().unwrap().tool.as_mut().unwrap();
    last.status = "running".into();
    last.args = json!("malformed consumer input is authoritative");
    last.effective_args = Some(json!({"accepted":[]}));
    store.save_read_model(&source, &model);
    let latest = store
        .saved_tool_state("opaque / exact")
        .unwrap()
        .tool
        .unwrap();
    assert_eq!(latest.status, "running");
    assert_eq!(
        latest.args,
        json!("malformed consumer input is authoritative")
    );
    assert_eq!(latest.effective_args, Some(json!({"accepted":[]})));
    for status in ["failed", "denied"] {
        model
            .transcript
            .last_mut()
            .unwrap()
            .tool
            .as_mut()
            .unwrap()
            .status = status.into();
        store.save_read_model(&source, &model);
        assert_eq!(
            store
                .saved_tool_state("opaque / exact")
                .unwrap()
                .tool
                .unwrap()
                .call_id,
            "accepted"
        );
    }
}

#[tokio::test]
async fn latest_tool_state_migrates_auxiliary_index_and_repairs_it_without_conversation_decode() {
    let (_root, _workspace, store) = fixture().await;
    let journal = std::fs::read(store.journal_path()).unwrap();
    let path = store.session_dir().join(".read-view.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "DROP TABLE keyed_records;
        DELETE FROM records WHERE section='tool-state-index';
        UPDATE records SET payload=x'30' WHERE section='conversation';",
    )
    .unwrap();
    drop(db);
    assert!(store.fresh_read_model().is_none());
    let latest = store.saved_tool_state("opaque / exact").unwrap();
    assert_eq!(latest.tool.unwrap().call_id, "accepted");
    assert!(
        store.saved_transcript_page(None, None).is_some(),
        "existing view format survives auxiliary migration"
    );
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "UPDATE keyed_records SET payload=x'30' WHERE section='tool-state'",
        [],
    )
    .unwrap();
    drop(db);
    assert_eq!(
        store
            .saved_tool_state("opaque / exact")
            .unwrap()
            .tool
            .unwrap()
            .call_id,
        "accepted"
    );
    assert_eq!(std::fs::read(store.journal_path()).unwrap(), journal);
}

#[tokio::test]
async fn latest_tool_state_never_repairs_missing_or_corrupt_source_from_canonical_history() {
    let (_root, _workspace, store) = fixture().await;
    let path = store.session_dir().join(".read-view.sqlite");
    std::fs::remove_file(&path).unwrap();
    let journal = std::fs::read(store.journal_path()).unwrap();
    assert!(store.saved_tool_state("opaque / exact").is_none());
    assert!(!path.exists());
    assert_eq!(std::fs::read(store.journal_path()).unwrap(), journal);
    store.project_read_model(render).unwrap();
    std::fs::write(store.journal_path(), b"{broken complete record}\n").unwrap();
    let broken = std::fs::read(store.journal_path()).unwrap();
    assert!(store.saved_tool_state("opaque / exact").is_none());
    assert_eq!(std::fs::read(store.journal_path()).unwrap(), broken);
}

#[tokio::test]
async fn latest_tool_state_rejects_corrupt_cached_rows_during_index_recovery() {
    let (_root, _workspace, store) = fixture().await;
    let db = rusqlite::Connection::open(store.session_dir().join(".read-view.sqlite")).unwrap();
    db.execute_batch(
        "DELETE FROM records WHERE section='tool-state-index';
        UPDATE records SET payload=x'30' WHERE section='transcript' AND position=0;",
    )
    .unwrap();
    drop(db);
    let journal = std::fs::read(store.journal_path()).unwrap();
    assert!(store.saved_tool_state("opaque / exact").is_none());
    assert_eq!(std::fs::read(store.journal_path()).unwrap(), journal);
}

#[tokio::test]
async fn latest_tool_state_auxiliary_upgrade_keeps_certified_append_incremental() {
    let (_root, _workspace, store) = fixture().await;
    store
        .project_read_view(render, journal_transcript_append_from_source)
        .unwrap();
    let db = rusqlite::Connection::open(store.session_dir().join(".read-view.sqlite")).unwrap();
    db.execute_batch(
        "DROP TABLE keyed_records; DELETE FROM records WHERE section='tool-state-index';",
    )
    .unwrap();
    drop(db);
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(MessageRole::User, "new tail")],
        )
        .await
        .unwrap();
    let journal = std::fs::read(store.journal_path()).unwrap();
    store
        .project_read_view(
            |_| panic!("auxiliary index upgrade must keep append incremental"),
            journal_transcript_append_from_source,
        )
        .unwrap();
    assert_eq!(
        store
            .saved_tool_state("opaque / exact")
            .unwrap()
            .tool
            .unwrap()
            .call_id,
        "accepted"
    );
    assert_eq!(std::fs::read(store.journal_path()).unwrap(), journal);
}

#[tokio::test]
async fn latest_tool_state_same_count_key_damage_cannot_be_reported_as_absence() {
    let (_root, _workspace, store) = fixture().await;
    let journal = std::fs::read(store.journal_path()).unwrap();
    let db = rusqlite::Connection::open(store.session_dir().join(".read-view.sqlite")).unwrap();
    db.execute("UPDATE keyed_records SET key='renamed' WHERE section='tool-state' AND key='opaque / exact'", []).unwrap();
    // Cardinality and payload/checksum remain unchanged. A negative lookup
    // must detect the damaged membership and repair from source-valid rows.
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM keyed_records WHERE section='tool-state'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    drop(db);
    assert_eq!(
        store
            .saved_tool_state("opaque / exact")
            .unwrap()
            .tool
            .unwrap()
            .call_id,
        "accepted"
    );
    assert!(store.saved_tool_state("renamed").unwrap().tool.is_none());
    assert_eq!(std::fs::read(store.journal_path()).unwrap(), journal);
}

#[tokio::test]
async fn latest_tool_state_manifest_binds_payload_identity_and_negative_reads_stay_compact() {
    let (_root, _workspace, store) = fixture().await;
    let path = store.session_dir().join(".read-view.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    let bytes: Vec<u8> = db
        .query_row(
            "SELECT payload FROM keyed_records WHERE section='tool-state'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut payload: Value = serde_json::from_slice(&bytes).unwrap();
    payload["name"] = json!("incorrect payload name");
    let bytes = serde_json::to_vec(&payload).unwrap();
    let checksum = ring::digest::digest(&ring::digest::SHA256, &bytes);
    db.execute(
        "UPDATE keyed_records SET payload=?1,checksum=?2 WHERE section='tool-state'",
        rusqlite::params![bytes, checksum.as_ref()],
    )
    .unwrap();
    drop(db);
    assert_eq!(
        store
            .saved_tool_state("opaque / exact")
            .unwrap()
            .tool
            .unwrap()
            .name,
        "opaque / exact"
    );
    // Once membership is verified, absence must not scan the entire display.
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "UPDATE records SET payload=x'30' WHERE section='transcript' AND position=0",
        [],
    )
    .unwrap();
    drop(db);
    assert!(store.saved_tool_state("missing").unwrap().tool.is_none());
}
