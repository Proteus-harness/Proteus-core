//! Opt-in, synthetic measurements; timings are reported, never asserted.
use super::*;
use std::{hint::black_box, time::Instant};

#[test]
fn discarded_transaction_cannot_publish_record_ids_parts_or_history() {
    let session_id = new_session_id();
    let thread_id = new_thread_id();
    let seed = CanonicalMessage::text(MessageRole::User, "committed");
    let mut state = JournalValidationState::default();
    state
        .apply(&record(
            session_id,
            None,
            Some(thread_id),
            None,
            1,
            JournalEntry::HistoryMutated(HistoryMutated {
                previous_revision: 0,
                new_revision: 1,
                mutation: HistoryMutationKind::Append,
                messages: vec![seed.clone()],
                compaction: None,
                tool_results: Vec::new(),
            }),
        ))
        .unwrap();
    let new_message = CanonicalMessage::text(MessageRole::User, "uncommitted candidate");
    let mut changed_seed = seed.clone();
    changed_seed.parts[0].payload = crate::model_standard::ContentPart::Text {
        text: "conflict".into(),
    };
    let mut candidate_record = record(
        session_id,
        None,
        Some(thread_id),
        None,
        2,
        JournalEntry::HistoryMutated(HistoryMutated {
            previous_revision: 1,
            new_revision: 2,
            mutation: HistoryMutationKind::Append,
            messages: vec![new_message.clone(), changed_seed],
            compaction: None,
            tool_results: Vec::new(),
        }),
    );
    let mut rejected = state.clone();
    assert!(rejected.apply(&candidate_record).is_err());
    let mut independently_committed = new_message;
    independently_committed.parts[0].payload = crate::model_standard::ContentPart::Text {
        text: "accepted".into(),
    };
    if let JournalEntry::HistoryMutated(mutation) = &mut candidate_record.entry {
        mutation.messages = vec![independently_committed.clone()];
    }
    // Reusing the candidate's record/part IDs must remain valid in the original.
    let unchanged = state.clone();
    state.apply(&candidate_record).unwrap();
    assert_eq!(unchanged.history(), &[seed.clone()]);
    assert_eq!(unchanged.history_revision(), 1);
    assert_eq!(state.history(), &[seed, independently_committed]);
    assert_eq!(state.history_revision(), 2);
    assert!(state.apply(&candidate_record).is_err());
}

#[tokio::test]
#[ignore = "synthetic warm writer versus legacy full reads; run with --nocapture"]
async fn execution_state_benchmark() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store =
        crate::core::SessionStore::new(root.path(), workspace.path(), new_session_id()).unwrap();
    let messages = (0..128)
        .map(|_| CanonicalMessage::text(MessageRole::User, "synthetic ".repeat(4096)))
        .collect::<Vec<_>>();
    store
        .append_history(new_thread_id(), None, &messages)
        .await
        .unwrap();
    let started = Instant::now();
    let legacy = (
        store.load_projection().unwrap().history_revision,
        store.load_projection().unwrap().interrupted_turns,
    );
    let legacy_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let current = (
        store.execution_state().await.unwrap().0,
        store.execution_state().await.unwrap().1,
    );
    let writer_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(legacy, current);
    eprintln!(
        "PERF_EXECUTION {}",
        serde_json::json!({"history_messages":128,"legacy_two_reads_ms":legacy_ms,"writer_state_ms":writer_ms})
    );
}

#[test]
#[ignore = "synthetic allocation/latency probe; run explicitly with --nocapture"]
fn projection_clone_benchmark() {
    let session_id = new_session_id();
    let execution_id = new_execution_id();
    let thread_id = new_thread_id();
    let mut state = JournalValidationState::default();
    let messages = (0..128)
        .map(|_| CanonicalMessage::text(MessageRole::User, "synthetic ".repeat(4096)))
        .collect::<Vec<_>>();
    state
        .apply(&record(
            session_id,
            None,
            Some(thread_id),
            None,
            1,
            JournalEntry::HistoryMutated(HistoryMutated {
                previous_revision: 0,
                new_revision: 1,
                mutation: HistoryMutationKind::Append,
                messages,
                compaction: None,
                tool_results: Vec::new(),
            }),
        ))
        .unwrap();
    for seq in 2..20_002 {
        state
            .apply(&record(
                session_id,
                Some(execution_id),
                None,
                None,
                seq,
                JournalEntry::ContextRequestRecorded(ContextRequestRecorded {
                    invocation_id: uuid::Uuid::new_v4(),
                    task: AgentTask::new("synthetic", "/tmp".into()),
                }),
            ))
            .unwrap();
    }
    let started = Instant::now();
    for seq in 20_002..20_258 {
        let fact = record(
            session_id,
            Some(execution_id),
            None,
            None,
            seq,
            JournalEntry::ContextRequestRecorded(ContextRequestRecorded {
                invocation_id: uuid::Uuid::new_v4(),
                task: AgentTask::new("synthetic", "/tmp".into()),
            }),
        );
        let mut next = black_box(&state).clone();
        next.apply(black_box(&fact)).unwrap();
        state = black_box(next);
    }
    eprintln!(
        "PERF_VALIDATION {}",
        serde_json::json!({
            "history_messages": state.history().len(), "seed_facts": 20_001,
            "transactions": 256, "clone_apply_ms": started.elapsed().as_secs_f64() * 1000.0,
        })
    );
    assert_eq!(state.history_revision(), 1);
    assert_eq!(state.history().len(), 128);
}
