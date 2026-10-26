use crate::model_standard::MessageRole;
use std::{fs::OpenOptions, io::Write, path::Path, sync::Arc};

use super::*;
use crate::domain::{
    Event, EventContext, EventEnvelope, HistoryCompactionReport, ModelRef, TokenUsageCategory,
    TokenUsageSnapshot, TokenUsageSource, ToolCall, ToolResult, new_session_id, new_thread_id,
    new_turn_id,
};

fn event(session: SessionId, sequence: u64, event: Event) -> EventEnvelope {
    let mut envelope = EventEnvelope::new(
        EventContext::new(session, new_thread_id(), Some(new_turn_id())),
        sequence,
        event,
    );
    envelope.timestamp_ms = sequence as i64;
    envelope
}

fn write_events(path: &Path, events: &[EventEnvelope], append: bool) {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(path)
        .unwrap();
    for event in events {
        serde_json::to_writer(&mut file, event).unwrap();
        writeln!(file).unwrap();
    }
}

fn input(path: &Path, session: SessionId) -> ContextMapInput {
    ContextMapInput {
        session_dir: Some(path.parent().unwrap().join("session")),
        session_id: Some(session),
        workspace_path: Some(path.parent().unwrap().into()),
        activity: None,
        history: vec![CanonicalMessage::text(MessageRole::User, "current history")],
        event_log_path: path.into(),
        diagnostics: vec!["caller diagnostic".into()],
    }
}

#[test]
fn telemetry_cache_reuses_ready_state_but_keeps_caller_history_and_activity_current() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.jsonl");
    let session = new_session_id();
    let mut report = HistoryCompactionReport::unchanged(4, Some("fixture".into()));
    report.summary = Some("private summary".into());
    report.metadata = serde_json::json!({"private": "metadata"});
    let usage = TokenUsageSnapshot::new(
        ModelRef::new("fixture", "model"),
        42,
        vec![TokenUsageCategory::new("history", 32).with_source(TokenUsageSource::Mixed)],
    )
    .with_phase("root")
    .with_source(TokenUsageSource::Provider);
    let events = vec![
        event(
            session,
            1,
            Event::ToolCallRequested {
                call: ToolCall::new("a", "zeta", serde_json::json!({})),
            },
        ),
        event(
            session,
            2,
            Event::ToolCallRequested {
                call: ToolCall::new("b", "alpha", serde_json::json!({})),
            },
        ),
        event(
            session,
            3,
            Event::ToolFinished {
                result: ToolResult::ok("a".into(), "output"),
            },
        ),
        event(
            session,
            4,
            Event::ContextBuilt {
                chunks: 3,
                token_estimate: Some(50),
            },
        ),
        event(session, 5, Event::TokenUsageUpdated { usage }),
        event(session, 6, Event::HistoryCompactionCompleted { report }),
        event(
            new_session_id(),
            7,
            Event::ContextBuilt {
                chunks: 999,
                token_estimate: None,
            },
        ),
    ];
    write_events(&path, &events, false);
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    // Valid unterminated final JSON is accepted, malformed tail is diagnosed as before.
    write!(file, "\n{{\"incomplete\"").unwrap();
    let first = telemetry::read(&path, Some(session), None).unwrap();
    let warm = telemetry::read(&path, Some(session), None).unwrap();
    #[cfg(unix)]
    assert!(
        Arc::ptr_eq(&first, &warm),
        "warm read must reuse parsed telemetry"
    );
    assert_eq!(
        serde_json::to_value(first.as_ref()).unwrap(),
        serde_json::to_value(warm.as_ref()).unwrap()
    );
    assert_eq!(first.tools.requested, 2);
    assert_eq!(first.tools.finished, 1);
    assert_eq!(first.tools.names, ["alpha", "zeta"]);
    assert_eq!(first.latest_context.as_ref().unwrap().chunks, 3);
    let usage = first.latest_usage.as_ref().unwrap();
    assert_eq!(usage.estimated_input_tokens, 42);
    assert_eq!(usage.phase.as_deref(), Some("root"));
    assert_eq!(usage.source, "provider");
    assert_eq!(usage.categories[0].source.as_deref(), Some("mixed"));
    assert_eq!(usage.turn_id, events[4].turn_id);
    let compact = first.latest_compaction.as_ref().unwrap();
    assert_eq!(compact.status, "completed");
    assert!(compact.summary_present);
    assert!(compact.report.as_ref().unwrap().summary.is_none());
    assert_eq!(
        compact.report.as_ref().unwrap().metadata,
        serde_json::Value::Null
    );
    assert_eq!(first.diagnostics, ["skipped 1 malformed event log lines"]);
    let initial = build_context_map_snapshot(input(&path, session)).unwrap();
    let mut changed = input(&path, session);
    changed.activity = Some(AppSessionActivity::from_counts(1, 2, 0));
    changed.history.push(CanonicalMessage::text(
        MessageRole::Assistant,
        "new history",
    ));
    changed.diagnostics = vec!["different caller".into()];
    let changed = build_context_map_snapshot(changed).unwrap();
    assert_eq!(changed.history.messages, 2);
    assert_eq!(initial.history.messages, 1);
    assert_eq!(changed.activity.unwrap().pending_approvals, 2);
    assert_eq!(changed.latest_usage, initial.latest_usage);
    assert_eq!(changed.diagnostics[0], "different caller");
    assert_eq!(initial.diagnostics[0], "caller diagnostic");
}

#[test]
fn telemetry_cache_invalidates_append_replacement_truncation_and_missing_log() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.jsonl");
    let session = new_session_id();
    let original = event(
        session,
        1,
        Event::ContextBuilt {
            chunks: 1,
            token_estimate: None,
        },
    );
    write_events(&path, &[original], false);
    let initial = telemetry::read(&path, Some(session), None).unwrap();
    write_events(
        &path,
        &[
            event(
                new_session_id(),
                2,
                Event::ContextBuilt {
                    chunks: 99,
                    token_estimate: None,
                },
            ),
            event(
                session,
                3,
                Event::ContextBuilt {
                    chunks: 2,
                    token_estimate: Some(20),
                },
            ),
        ],
        true,
    );
    let appended = telemetry::read(&path, Some(session), None).unwrap();
    assert!(!Arc::ptr_eq(&initial, &appended));
    assert_eq!(appended.latest_context.as_ref().unwrap().chunks, 2);
    let replacement = directory.path().join("replacement");
    write_events(
        &replacement,
        &[event(
            session,
            4,
            Event::ContextBuilt {
                chunks: 3,
                token_estimate: None,
            },
        )],
        false,
    );
    std::fs::rename(replacement, &path).unwrap();
    assert_eq!(
        telemetry::read(&path, Some(session), None)
            .unwrap()
            .latest_context
            .as_ref()
            .unwrap()
            .chunks,
        3
    );
    std::fs::write(&path, "malformed\n\n{}").unwrap();
    let truncated = telemetry::read(&path, Some(session), None).unwrap();
    assert!(truncated.latest_context.is_none());
    assert_eq!(
        truncated.diagnostics,
        ["skipped 2 malformed event log lines"]
    );
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        telemetry::read(&path, Some(session), None)
            .unwrap()
            .diagnostics,
        ["event log not found; using session history only"]
    );
    // File recreation must not revive an old ready DTO under the same path.
    std::fs::write(&path, []).unwrap();
    assert!(
        telemetry::read(&path, Some(session), None)
            .unwrap()
            .latest_context
            .is_none()
    );
    std::fs::write(&path, [0xff, b'\n']).unwrap();
    assert!(telemetry::read(&path, Some(session), None).is_err());
}

#[cfg(unix)]
#[test]
fn telemetry_cache_detects_same_size_rewrite_even_with_restored_mtime() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.jsonl");
    let session = new_session_id();
    write_events(
        &path,
        &[event(
            session,
            1,
            Event::ContextBuilt {
                chunks: 1,
                token_estimate: None,
            },
        )],
        false,
    );
    let before = std::fs::metadata(&path).unwrap();
    let initial = telemetry::read(&path, Some(session), None).unwrap();
    let original = std::fs::read_to_string(&path).unwrap();
    let changed = original.replace("\"chunks\":1", "\"chunks\":9");
    assert_ne!(original, changed);
    assert_eq!(original.len(), changed.len());
    // Keep the test deterministic even on filesystems with coarser change timestamps.
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(&path, changed).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(before.modified().unwrap()))
        .unwrap();
    let changed = telemetry::read(&path, Some(session), None).unwrap();
    assert!(!Arc::ptr_eq(&initial, &changed));
    assert_eq!(changed.latest_context.as_ref().unwrap().chunks, 9);
}

#[test]
fn telemetry_preserves_log_order_compaction_transitions_and_path_only_matching() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.jsonl");
    let session = new_session_id();
    let session_dir = directory.path().join("session");
    let events = vec![
        event(
            session,
            90,
            Event::SessionStarted {
                session_id: session,
                cwd: directory.path().into(),
                model: None,
                session_dir: Some(session_dir.clone()),
            },
        ),
        event(
            session,
            80,
            Event::HistoryCompactionStarted {
                reason: None,
                input_messages: 2,
                token_estimate: None,
                trigger_tokens: None,
            },
        ),
        event(
            session,
            70,
            Event::HistoryCompactionFailed {
                reason: None,
                input_messages: 2,
                token_estimate: None,
                trigger_tokens: None,
                message: "failure".into(),
            },
        ),
        event(
            session,
            60,
            Event::ToolFinished {
                result: ToolResult::error("failed".into(), "error"),
            },
        ),
        event(
            session,
            50,
            Event::ContextBuilt {
                chunks: 4,
                token_estimate: None,
            },
        ),
        event(
            session,
            40,
            Event::ContextBuilt {
                chunks: 5,
                token_estimate: None,
            },
        ),
    ];
    write_events(&path, &events, false);
    // Valid JSON at EOF without a trailing newline remains a complete event.
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
    let telemetry = telemetry::read(&path, Some(session), Some(&session_dir)).unwrap();
    assert_eq!(telemetry.latest_context.as_ref().unwrap().chunks, 5);
    assert_eq!(
        telemetry.latest_compaction.as_ref().unwrap().status,
        "failed"
    );
    assert_eq!(telemetry.tools.finished, 1);
    assert_eq!(telemetry.tools.failed, 1);
    assert!(telemetry.diagnostics.is_empty());
    // The pre-existing path-only fallback matches SessionStarted itself; it must
    // not silently widen selection to the rest of that session's envelopes.
    let path_only = telemetry::read(&path, None, Some(&session_dir)).unwrap();
    assert!(path_only.latest_context.is_none());
    assert_eq!(path_only.tools.finished, 0);
    assert!(
        telemetry::read(&path, Some(new_session_id()), Some(&session_dir))
            .unwrap()
            .latest_context
            .is_none()
    );
}
