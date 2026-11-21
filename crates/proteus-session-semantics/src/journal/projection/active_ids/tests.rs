use super::super::*;
use crate::{
    domain::{new_message_id, new_record_id},
    journal::HistoryMutated,
    model_standard::MessageRole,
};

fn row(text: &str) -> CanonicalMessage {
    CanonicalMessage::text(MessageRole::User, text)
}

fn apply(
    state: &mut JournalValidationState,
    kind: HistoryMutationKind,
    messages: Vec<CanonicalMessage>,
) -> Result<()> {
    let revision = state.history_revision();
    state.apply(&JournalRecord {
        schema_version: JOURNAL_SCHEMA_VERSION,
        record_id: new_record_id(),
        session_seq: state.record_ids.len() as u64 + 1,
        timestamp_ms: 0,
        session_id: SessionId::from_u128(1),
        execution_id: None,
        thread_id: Some(ThreadId::from_u128(2)),
        turn_id: None,
        entry: JournalEntry::HistoryMutated(HistoryMutated {
            previous_revision: revision,
            new_revision: revision + 1,
            mutation: kind,
            messages,
            compaction: None,
            tool_results: vec![],
        }),
    })
}

#[test]
fn one_row_append_matches_full_active_identity_check() {
    let mut state = JournalValidationState::default();
    let first = row("first");
    let second = row("second");
    apply(
        &mut state,
        HistoryMutationKind::Replace,
        vec![first.clone(), second.clone()],
    )
    .unwrap();
    let mut same_part = first.clone();
    same_part.id = new_message_id();
    let mut both = same_part.clone();
    both.id = second.id;
    let mut empty = row("empty");
    empty.parts.clear();
    let mut empty_duplicate = empty.clone();
    empty_duplicate.id = first.id;
    let mut multipart = row("multipart");
    multipart.parts.push(first.parts[0].clone());
    for candidate in [
        row("new"),
        first.clone(),
        same_part,
        both,
        empty,
        empty_duplicate,
        multipart,
    ] {
        let mut full = state.history().to_vec();
        full.push(candidate.clone());
        let expected = full_check(&full).map_err(|e| e.to_string());
        let mut actual = state.clone();
        let got = apply(&mut actual, HistoryMutationKind::Append, vec![candidate])
            .map_err(|e| e.to_string());
        assert_eq!(got, expected);
        assert_eq!(actual.history(), full);
    }
    // A replacement can remove active IDs; historical use alone isn't an error.
    apply(&mut state, HistoryMutationKind::Replace, vec![second]).unwrap();
    apply(&mut state, HistoryMutationKind::Append, vec![first]).unwrap();
}

#[test]
fn failed_state_cannot_recertify_an_invalid_prefix_by_appending() {
    let mut state = JournalValidationState::default();
    let a = row("a");
    apply(&mut state, HistoryMutationKind::Append, vec![a.clone()]).unwrap();
    let original = apply(&mut state, HistoryMutationKind::Append, vec![a])
        .unwrap_err()
        .to_string();
    let repeated = apply(&mut state, HistoryMutationKind::Append, vec![row("b")])
        .unwrap_err()
        .to_string();
    assert_eq!(repeated, original);
    apply(&mut state, HistoryMutationKind::Replace, vec![row("reset")]).unwrap();
    apply(&mut state, HistoryMutationKind::Append, vec![row("good")]).unwrap();
}

#[test]
fn batch_append_matches_first_duplicate_in_full_check() {
    let mut state = JournalValidationState::default();
    let prefix: Vec<_> = (0..24).map(|i| row(&format!("p{i}"))).collect();
    apply(&mut state, HistoryMutationKind::Replace, prefix.clone()).unwrap();
    let fresh = row("fresh");
    let mut old_part = row("other");
    old_part.parts.push(prefix[2].parts[0].clone());
    let mut old_message = row("different");
    old_message.id = prefix[1].id;
    let mut internal_parts = row("multi");
    internal_parts.parts.push(internal_parts.parts[0].clone());
    let mut empty = row("empty");
    empty.parts.clear();
    for a in [&fresh, &old_part, &old_message, &internal_parts, &empty] {
        for b in [&fresh, &old_part, &old_message, &internal_parts, &empty] {
            let added = vec![a.clone(), b.clone()];
            let mut full = prefix.clone();
            full.extend(added.iter().cloned());
            let expected = full_check(&full).map_err(|e| e.to_string());
            let mut branch = state.clone();
            let actual =
                apply(&mut branch, HistoryMutationKind::Append, added).map_err(|e| e.to_string());
            assert_eq!(actual, expected);
            assert_eq!(branch.history(), full);
            if let Err(err) = expected {
                let got = apply(
                    &mut branch,
                    HistoryMutationKind::Append,
                    vec![row("a"), row("b")],
                )
                .unwrap_err();
                assert_eq!(err, got.to_string());
            }
        }
    }
}

// Frozen full-scan oracle: compare exact first failures as well as acceptance.
fn full_check(messages: &[CanonicalMessage]) -> Result<()> {
    let mut message_ids = std::collections::HashSet::new();
    let mut part_ids = std::collections::HashSet::new();
    for message in messages {
        if !message_ids.insert(message.id) {
            bail!(
                "active history contains duplicate message id {}",
                message.id
            );
        }
        for part in &message.parts {
            if !part_ids.insert(part.part_id) {
                bail!(
                    "active history contains duplicate part id {} at message {}",
                    part.part_id,
                    message.id,
                );
            }
        }
    }
    Ok(())
}

#[test]
fn branches_and_replacement_follow_active_membership_not_all_known_parts() {
    use HistoryMutationKind::{Append, Replace};
    let shared = row("shared");
    let removed = row("removed");
    let added = row("left branch");
    let mut left = JournalValidationState::default();
    apply(&mut left, Append, vec![shared.clone(), removed.clone()]).unwrap();
    let mut right = left.clone();
    apply(&mut left, Append, vec![added.clone()]).unwrap();
    apply(&mut right, Replace, vec![shared.clone()]).unwrap();
    // Both the previously removed stable identities and an independent branch's
    // new identities are legal here; neither is currently active on the right.
    apply(&mut right, Append, vec![removed.clone(), added]).unwrap();
    let expected = format!(
        "active history contains duplicate message id {}",
        removed.id
    );
    assert_eq!(
        apply(&mut left, Append, vec![removed.clone()])
            .unwrap_err()
            .to_string(),
        expected
    );
    assert_eq!(
        apply(&mut right, Append, vec![removed])
            .unwrap_err()
            .to_string(),
        expected
    );
}
