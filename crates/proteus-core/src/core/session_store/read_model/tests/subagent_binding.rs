//! Background linkage is journal-owned even when its source turn already ended.
use super::{incremental::*, *};
use crate::{
    contracts::ToolExecutionRecorder,
    core::SessionToolExecutionRecorder,
    domain::{SubagentSessionBinding, ToolCall, ToolCallResolution, ToolResult},
};

async fn source_call(store: &SessionStore, workspace: &Path) -> (ExecutionAttribution, ToolCall) {
    let attribution = ExecutionAttribution::for_turn(
        new_execution_id(),
        store.session_id(),
        new_thread_id(),
        new_turn_id(),
    );
    store
        .append_execution_journal_entry(
            attribution,
            JournalEntry::TurnOpened(TurnOpened {
                intent: None,
                task: AgentTask::new("spawn background child", workspace.into()),
                base_history_revision: store.load_projection().unwrap().history_revision,
                module_epoch: 0,
                config_snapshot: serde_json::json!({}),
            }),
        )
        .await
        .unwrap();
    let call = ToolCall::new(
        "spawn-source",
        "spawn_agent",
        serde_json::json!({"task_name":"child"}),
    );
    let recorder = SessionToolExecutionRecorder::new(store.clone());
    recorder
        .tool_call_requested(attribution, OperationOwner::Workflow, &call)
        .await
        .unwrap();
    recorder
        .tool_call_resolved(attribution, &call, &ToolCallResolution::Allowed)
        .await
        .unwrap();
    recorder
        .tool_result_recorded(
            attribution,
            &ToolResult::ok(call.id.clone(), "queued")
                .with_metadata(serde_json::json!({"tool":"spawn_agent"})),
        )
        .await
        .unwrap();
    let agent = attribution.agent.unwrap();
    store
        .append_journal_entry(
            agent.thread_id,
            Some(agent.turn_id),
            JournalEntry::TurnSettled(TurnSettled {
                status: TurnSettlementStatus::Success,
                output: None,
                error: None,
            }),
        )
        .await
        .unwrap();
    (attribution, call)
}

fn binding(call: &ToolCall) -> SubagentSessionBinding {
    SubagentSessionBinding {
        source_call_id: call.id.clone(),
        child_thread_id: new_thread_id(),
        session_id: new_session_id(),
        session_dir: "/saved/exact child".into(),
    }
}

#[tokio::test]
async fn late_subagent_binding_reprojects_original_row_and_preserves_canonical_history() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "parent prompt").await;
    let (attribution, call) = source_call(&store, workspace.path()).await;
    let before = project(&store);
    let old_revision = before.cursor.unwrap().revision;
    let before_history = store.load_messages().unwrap();
    let before_usage = store.usage_snapshot().await.unwrap();
    let prefix = fs::read(store.journal_path()).unwrap();
    let bound = binding(&call);
    SessionToolExecutionRecorder::new(store.clone())
        .subagent_session_bound(attribution, &bound)
        .await
        .unwrap();
    let complete = fs::read(store.journal_path()).unwrap();
    assert!(
        complete.starts_with(&prefix),
        "linkage only appends an immutable fact"
    );
    let full_rendered = Cell::new(false);
    let view = store
        .project_read_view(
            |source| {
                full_rendered.set(true);
                render(source)
            },
            crate::app_server::journal_transcript_append_from_source,
        )
        .unwrap();
    assert!(
        full_rendered.get(),
        "a late binding updates an old row, never an independent append"
    );
    let expected = oracle(&store);
    assert_display_view(&view, &expected);
    assert_same_view(&store.fresh_read_model().unwrap(), &expected);
    assert_eq!(store.load_messages().unwrap(), before_history);
    assert_eq!(store.usage_snapshot().await.unwrap(), before_usage);
    let tools: Vec<_> = expected
        .transcript
        .iter()
        .filter_map(|row| row.tool.as_ref())
        .collect();
    assert_eq!(tools.len(), 1, "the source tool remains the sole row");
    assert_eq!(tools[0].call_id, call.id);
    assert_eq!(tools[0].result.as_deref(), Some("queued"));
    assert_eq!(tools[0].metadata["tool"], "spawn_agent");
    assert_eq!(
        tools[0].metadata["child_thread_id"],
        serde_json::json!(bound.child_thread_id)
    );
    assert_eq!(
        tools[0].metadata["subagent_session"],
        serde_json::json!({
            "session_id":bound.session_id,"session_dir":bound.session_dir,
        })
    );
    assert!(
        store
            .saved_transcript_page(None, Some(&old_revision))
            .is_none(),
        "old paging revision cannot hide a changed source row"
    );
    assert_eq!(fs::read(store.journal_path()).unwrap(), complete);
    // The cold, collected oracle and streaming JSONL passes agree as well.
    let rows = journal_transcript_messages(store.load_records().unwrap(), None);
    assert_eq!(
        serde_json::to_value(rows).unwrap(),
        serde_json::to_value(expected.transcript).unwrap()
    );
}

#[tokio::test]
async fn subagent_binding_rejects_wrong_owner_unknown_denied_and_conflicting_links() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "parent prompt").await;
    let (attribution, call) = source_call(&store, workspace.path()).await;
    let recorder = SessionToolExecutionRecorder::new(store.clone());
    let bound = binding(&call);
    let unchanged = fs::read(store.journal_path()).unwrap();
    let original_records = store.load_records().unwrap();
    let mut foreign = attribution;
    foreign.execution_id = new_execution_id();
    assert!(
        recorder
            .subagent_session_bound(foreign, &bound)
            .await
            .is_err()
    );
    let mut invalid = bound.clone();
    invalid.source_call_id = "unknown-call".into();
    assert!(
        recorder
            .subagent_session_bound(attribution, &invalid)
            .await
            .is_err()
    );
    invalid = bound.clone();
    invalid.session_dir = "relative-child".into();
    assert!(
        recorder
            .subagent_session_bound(attribution, &invalid)
            .await
            .is_err()
    );
    assert_eq!(fs::read(store.journal_path()).unwrap(), unchanged);
    recorder
        .subagent_session_bound(attribution, &bound)
        .await
        .unwrap();
    let committed = fs::read(store.journal_path()).unwrap();
    invalid = bound.clone();
    invalid.session_id = new_session_id();
    assert!(
        recorder
            .subagent_session_bound(attribution, &invalid)
            .await
            .is_err()
    );
    assert!(
        recorder
            .subagent_session_bound(attribution, &bound)
            .await
            .is_err(),
        "one immutable fact per source invocation"
    );
    assert_eq!(fs::read(store.journal_path()).unwrap(), committed);

    // A source call that was denied cannot acquire a child link, even though
    // its request and owner are otherwise valid and its turn is now settled.
    let mut denied = original_records;
    for record in &mut denied {
        if let JournalEntry::ToolCallRecorded(tool) = &mut record.entry
            && matches!(
                tool.phase,
                crate::core::ToolCallRecordPhase::Resolved { .. }
            )
        {
            tool.phase = crate::core::ToolCallRecordPhase::Resolved {
                resolution: ToolCallResolution::PolicyDenied {
                    reason: "denied".into(),
                },
            };
        }
    }
    let mut record = denied.last().unwrap().clone();
    record.record_id = crate::domain::new_record_id();
    record.execution_id = Some(attribution.execution_id);
    record.session_seq += 1;
    record.entry = JournalEntry::SubagentSessionBoundRecorded(bound);
    denied.push(record);
    let error = crate::core::JournalProjection::build(store.session_id(), denied).unwrap_err();
    assert!(
        error.to_string().contains("ownership or admission"),
        "{error:#}"
    );
}

#[tokio::test]
async fn source_result_and_binding_fact_must_agree_in_either_arrival_order() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "parent prompt").await;
    let (attribution, call) = source_call(&store, workspace.path()).await;
    let bound = binding(&call);
    let original = store.load_records().unwrap();
    for early in [true, false] {
        for conflicting in [false, true] {
            let mut records = original.clone();
            let mut reported = bound.clone();
            if conflicting {
                reported.session_id = new_session_id();
            }
            for record in &mut records {
                if let JournalEntry::ToolResultRecorded(tool) = &mut record.entry {
                    tool.result.metadata["child_thread_id"] =
                        serde_json::json!(reported.child_thread_id);
                    tool.result.metadata["subagent_session"] = serde_json::json!({
                        "session_id":reported.session_id,"session_dir":reported.session_dir,
                    });
                }
            }
            let mut record = records.last().unwrap().clone();
            record.record_id = crate::domain::new_record_id();
            record.execution_id = Some(attribution.execution_id);
            record.entry = JournalEntry::SubagentSessionBoundRecorded(bound.clone());
            let position = if early {
                records
                    .iter()
                    .position(|record| matches!(record.entry, JournalEntry::ToolResultRecorded(_)))
                    .unwrap()
            } else {
                records.len()
            };
            records.insert(position, record);
            for (index, record) in records.iter_mut().enumerate() {
                record.session_seq = index as u64 + 1;
            }
            let result = crate::core::JournalProjection::build(store.session_id(), records);
            assert_eq!(
                result.is_err(),
                conflicting,
                "early={early}, conflicting={conflicting}: {result:?}"
            );
        }
    }
}

#[test]
fn subagent_binding_kind_decodes_strict_payload_in_both_storage_encodings() {
    let call = ToolCall::new("source", "spawn_agent", serde_json::json!({}));
    let entry = JournalEntry::SubagentSessionBoundRecorded(binding(&call));
    let payload = entry.payload_value().unwrap();
    assert_eq!(
        JournalEntry::from_kind_and_payload(entry.kind(), payload.clone()).unwrap(),
        entry
    );
    assert_eq!(
        JournalEntry::from_kind_and_bytes(entry.kind(), &serde_json::to_vec(&payload).unwrap())
            .unwrap(),
        entry
    );
    let mut extra = payload.clone();
    extra["unknown"] = true.into();
    assert!(JournalEntry::from_kind_and_payload(entry.kind(), extra).is_err());
    let mut missing = payload;
    missing.as_object_mut().unwrap().remove("source_call_id");
    assert!(
        JournalEntry::from_kind_and_bytes(entry.kind(), &serde_json::to_vec(&missing).unwrap())
            .is_err()
    );
}
