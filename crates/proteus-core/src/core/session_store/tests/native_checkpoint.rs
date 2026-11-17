use super::*;
use crate::model_standard::{
    CanonicalPart, ContentPart, ModelOperation, PartProvenance, PartScope,
};

#[tokio::test]
async fn native_checkpoint_reopens_in_order_without_displaying_or_rewriting_opaque_data() {
    let config_dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = test_store(config_dir.path(), workspace.path());
    let thread = new_thread_id();
    let user = CanonicalMessage::text(MessageRole::User, "retained real user");
    store
        .append_history(thread, None, std::slice::from_ref(&user))
        .await
        .unwrap();
    let payload = serde_json::json!({
        "type":"compaction",
        "encrypted_content":"SYNTHETIC-opaque-bytes-+/_=\nUnicode-Ж",
        "id":"provider-item-kept-exactly"
    });
    let checkpoint = CanonicalMessage::from_parts(
        MessageRole::Assistant,
        vec![CanonicalPart::new(
            PartProvenance::Compactor,
            PartScope::Conversation,
            ContentPart::Opaque {
                format: "org.example/native-checkpoint.v1".into(),
                data: payload.clone(),
            },
        )],
    );
    assert!(checkpoint.display_text().is_empty());
    let replacement = vec![user.clone(), checkpoint.clone()];
    let mut report = changed_report(1, 2);
    report.summary = None;
    store
        .replace_history(thread, None, &replacement, Some(report))
        .await
        .unwrap();
    let session_dir = store.session_dir().to_path_buf();
    drop(store);
    let reopened = SessionStore::open(session_dir).unwrap();
    assert_eq!(reopened.load_messages().unwrap(), replacement);
    let continuation = CanonicalMessage::text(MessageRole::User, "continue after reopening");
    reopened
        .append_history(thread, None, std::slice::from_ref(&continuation))
        .await
        .unwrap();
    let restored = reopened.load_messages().unwrap();
    assert_eq!(restored, [user, checkpoint, continuation]);
    let request =
        CanonicalModelRequest::new(crate::domain::ModelRef::new("fixture", "native"), restored)
            .with_operation(ModelOperation::Compact);
    let decoded: CanonicalModelRequest =
        serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
    assert_eq!(decoded, request);
    assert!(
        matches!(&decoded.messages[1].parts[0].payload, ContentPart::Opaque { data, .. } if data == &payload)
    );
}
