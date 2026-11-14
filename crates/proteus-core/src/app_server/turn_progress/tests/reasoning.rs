use super::*;

#[test]
fn readable_reasoning_keeps_item_identity_separate_from_answer_and_reconciles_completion() {
    let mut progress = TurnProgress::default();
    let id = uuid::Uuid::from_u128(7);
    apply(
        &mut progress,
        Event::TurnStarted {
            session_id: new_session_id(),
            thread_id: root_thread_id(),
            turn_id: new_turn_id(),
        },
    );
    let delta = |offset, text: &str| Event::AssistantReasoningTextDelta {
        message_id: id,
        offset,
        text: text.into(),
    };
    apply(&mut progress, delta(0, "Мы"));
    apply(&mut progress, delta(0, "Мы"));
    apply(&mut progress, delta("Мы".len(), "сль"));
    progress.apply(&envelope(child_thread_id(), delta("Мысль".len(), "чужая")));
    apply(
        &mut progress,
        Event::AssistantTextDelta {
            message_id: id,
            phase: None,
            offset: 0,
            text: "answer".into(),
        },
    );
    let rows = progress.snapshot();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (&rows[0].role, rows[0].text.as_str()),
        (&"reasoning".to_owned(), "Мысль")
    );
    assert_eq!(rows[1].text, "answer");
    assert_eq!(rows[0].message_id, rows[1].message_id);
    let at = rows[0].timestamp_ms;
    apply(
        &mut progress,
        Event::AssistantReasoningMessageCompleted {
            message_id: id,
            text: "Полная мысль".into(),
        },
    );
    apply(
        &mut progress,
        Event::AssistantReasoningMessageCompleted {
            message_id: id,
            text: "Полная мысль".into(),
        },
    );
    apply(
        &mut progress,
        Event::AssistantMessageCompleted {
            message_id: id,
            phase: None,
            text: "answer final".into(),
        },
    );
    let rows = progress.snapshot();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].text, "Полная мысль");
    assert_eq!(rows[0].timestamp_ms, at);
    assert!(!rows[0].streaming);
    assert_eq!(rows[1].text, "answer final");
    apply(
        &mut progress,
        Event::AssistantReasoningTextDelta {
            message_id: uuid::Uuid::from_u128(8),
            offset: 0,
            text: "next request".into(),
        },
    );
    assert_eq!(progress.snapshot().len(), 3);
    progress.finish_parent_turn();
    assert!(progress.snapshot().is_empty());
}

#[test]
fn completed_reasoning_without_live_deltas_precedes_answer_and_empty_completion_removes_it() {
    let mut progress = TurnProgress::default();
    let id = uuid::Uuid::from_u128(1);
    apply(&mut progress, delta_for(1, "answer"));
    apply(
        &mut progress,
        Event::AssistantReasoningMessageCompleted {
            message_id: id,
            text: "readable".into(),
        },
    );
    assert_eq!(progress.snapshot()[0].role, "reasoning");
    apply(
        &mut progress,
        Event::AssistantReasoningMessageCompleted {
            message_id: id,
            text: "   ".into(),
        },
    );
    assert_eq!(progress.snapshot().len(), 1);
    assert_eq!(progress.snapshot()[0].role, "assistant");
}
