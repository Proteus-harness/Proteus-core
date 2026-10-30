use super::*;

#[tokio::test]
async fn consuming_render_keeps_non_transcript_sections_and_accepts_local_closures() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "owned projection").await;
    add_exchange(&store, workspace.path()).await;
    let projection = store.load_projection().unwrap();
    let expected_conversation = ConversationSnapshot {
        revision: projection.history_revision,
        messages: projection.history.clone(),
        model_context: ModelContextState::from_records(&projection.records, None).snapshot(),
    };
    let mut usage = UsageProjection::default();
    for record in &projection.records {
        usage.apply(record);
    }
    let expected_summary = crate::core::session_store::catalog::session_summary_from_projection(
        store.session_dir().to_owned(),
        store.session_id(),
        &projection,
    )
    .unwrap();
    let expected_rows = render(projection.records.clone());
    let expected_ids = projection
        .records
        .iter()
        .map(|record| record.record_id)
        .collect::<Vec<_>>();
    let calls = Cell::new(0);
    let model = store
        .project_read_model(|records| {
            calls.set(calls.get() + 1);
            assert_eq!(
                records
                    .iter()
                    .map(|record| record.record_id)
                    .collect::<Vec<_>>(),
                expected_ids,
            );
            render(records)
        })
        .unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(model.conversation, expected_conversation);
    assert_eq!(model.usage, usage.snapshot(store.session_id()));
    assert_eq!(model.summary, expected_summary);
    assert_eq!(
        serde_json::to_value(model.transcript).unwrap(),
        serde_json::to_value(expected_rows).unwrap(),
    );
}

#[tokio::test]
async fn consuming_render_still_precedes_summary_metadata_observation() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "metadata order").await;
    let bytes = fs::read(store.journal_path()).unwrap();
    let modified_ms = 1_700_000_000_000_u64;
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_millis(modified_ms);
    let calls = Cell::new(0);
    let model = store
        .project_read_model(|records| {
            calls.set(calls.get() + 1);
            fs::File::open(store.journal_path())
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(modified))
                .unwrap();
            render(records)
        })
        .unwrap();
    assert_eq!(calls.get(), 1);
    let observed_ms = fs::metadata(store.journal_path())
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    assert_eq!(observed_ms, modified_ms);
    assert_eq!(model.summary.updated_at_ms, Some(observed_ms));
    assert_eq!(fs::read(store.journal_path()).unwrap(), bytes);
    // The callback changed source metadata, so the existing fingerprint gate
    // must not mark this in-memory result as a newly persisted fresh view.
    assert!(store.fresh_read_model().is_none());
}

#[tokio::test]
async fn consuming_render_still_precedes_fallible_workspace_resolution() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "workspace order").await;
    let rendered = Cell::new(false);
    let result = store.project_read_model(|records| {
        rendered.set(true);
        fs::remove_dir(workspace.path()).unwrap();
        render(records)
    });
    assert!(
        rendered.get(),
        "summary filesystem checks must remain after render"
    );
    assert!(
        result.is_err(),
        "the vanished workspace must still fail summary resolution"
    );
}
