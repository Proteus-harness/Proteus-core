use super::super::super::types::HistoryMutated;
use super::*;
use crate::{
    domain::{new_record_id, new_session_id},
    model_standard::{ContentPart, MessageRole},
};

fn append(session_id: SessionId, previous: u64, messages: Vec<CanonicalMessage>) -> JournalRecord {
    JournalRecord {
        schema_version: JOURNAL_SCHEMA_VERSION,
        record_id: new_record_id(),
        session_id,
        session_seq: previous + 1,
        timestamp_ms: previous as i64,
        execution_id: None,
        thread_id: None,
        turn_id: None,
        entry: JournalEntry::HistoryMutated(HistoryMutated {
            previous_revision: previous,
            new_revision: previous + 1,
            mutation: HistoryMutationKind::Append,
            messages,
            compaction: None,
            tool_results: Vec::new(),
        }),
    }
}

fn user(text: &str) -> CanonicalMessage {
    CanonicalMessage::text(MessageRole::User, text)
}

#[test]
fn read_fold_observes_the_intact_append_then_transfers_original_history() {
    let session = new_session_id();
    let mut fold = JournalValidationState::default();
    let mut sequence = 1;
    let mut observed = 0;
    for i in 0..2 {
        let m = user("stored 🦀 message");
        let original_parts = m.parts.as_ptr();
        let mut rows = Vec::with_capacity(1);
        rows.push(m.clone());
        let exact_outer = rows.as_ptr();
        let moved_parts = rows[0].parts.as_ptr();
        assert_ne!(moved_parts, original_parts);
        let record = append(session, i, rows);
        fold.apply_sequenced_for_read(session, record, &mut sequence, |record| {
            let JournalEntry::HistoryMutated(appended) = &record.entry else {
                panic!("expected an append");
            };
            assert_eq!(appended.messages, [m]);
            assert_eq!(appended.messages.as_ptr(), exact_outer);
            observed += 1;
        })
        .unwrap();
        if i == 0 {
            assert_eq!(fold.history().as_ptr(), exact_outer);
        }
        assert_eq!(fold.history()[i as usize].parts.as_ptr(), moved_parts);
        assert_eq!(fold.history_revision(), i + 1);
    }
    assert_eq!(sequence, 3);
    assert_eq!(observed, 2);
    assert_eq!(fold.history().len(), 2);
}

#[test]
fn read_fold_failure_keeps_competing_error_order_and_never_observes_rejected_rows() {
    let session = new_session_id();
    let existing = user("already committed");
    let r0 = append(session, 0, vec![existing.clone()]);
    for mode in 0..6 {
        let duplicate = existing.clone();
        let mut last = user("later unrelated");
        // A duplicate message id must lose to an earlier global scope/stable
        // part check, but win when both issues are distinct active collisions.
        if mode == 0 {
            last.parts[0].scope = PartScope::Trace;
        }
        if mode == 1 {
            last.parts[0].part_id = existing.parts[0].part_id;
            last.parts[0].payload = ContentPart::Text {
                text: "changed".into(),
            };
        }
        if mode == 2 {
            last.parts[0] = existing.parts[0].clone();
        }
        let mut candidate = append(session, 1, vec![duplicate, last]);
        if mode == 3 {
            candidate.record_id = r0.record_id;
        }
        if mode == 4 {
            candidate.session_seq = 3;
        }
        if mode == 5 {
            candidate.schema_version += 1;
        }
        let mut stock = JournalValidationState::default();
        let mut seq = 1;
        stock.apply_sequenced(session, &r0, &mut seq).unwrap();
        let expected = stock
            .apply_sequenced(session, &candidate, &mut seq)
            .unwrap_err()
            .to_string();
        assert_eq!(seq, 2);

        let mut owned = JournalValidationState::default();
        let mut seq = 1;
        let mut seen = 0;
        owned
            .apply_sequenced_for_read(session, r0.clone(), &mut seq, |_| seen += 1)
            .unwrap();
        let got = owned
            .apply_sequenced_for_read(session, candidate, &mut seq, |_| seen += 1)
            .unwrap_err()
            .to_string();
        assert_eq!(got, expected);
        assert_eq!(seq, 2);
        assert_eq!(seen, 1);
    }
}
