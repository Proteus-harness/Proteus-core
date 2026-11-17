use super::*;
use crate::model_standard::MessageRole;
use serde_json::json;

fn call_history(calls: &[ToolCall]) -> Vec<CanonicalMessage> {
    vec![CanonicalMessage::new(
        MessageRole::Assistant,
        calls
            .iter()
            .cloned()
            .map(|call| ContentPart::ToolCall { call })
            .collect(),
    )]
}

fn capture_error(history: &[CanonicalMessage], bindings: &[WorkflowToolResultBinding]) -> String {
    HistoryCapture::new(history, bindings)
        .unwrap_err()
        .to_string()
}

#[test]
fn history_lookup_retains_missing_ambiguous_and_result_checks() {
    let call = ToolCall::new("bound", "probe", json!({}));
    let binding = WorkflowToolResultBinding::new(call.clone());
    let mut history = call_history(std::slice::from_ref(&call));
    let result = CanonicalMessage::new(
        MessageRole::Tool,
        vec![ContentPart::ToolResult {
            result: ToolResult::ok(call.id.clone(), "recorded"),
        }],
    );
    assert_eq!(
        capture_error(
            std::slice::from_ref(&result),
            std::slice::from_ref(&binding)
        ),
        "checkpoint binding bound has no history call"
    );
    history.push(result);
    assert_eq!(
        capture_error(&history, std::slice::from_ref(&binding)),
        "checkpoint binding already has a history result"
    );
    // A duplicate in a different message must not be overwritten by the index;
    // ambiguity still takes precedence over an already-present result.
    history.extend(call_history(&[ToolCall::new(
        "bound",
        "other",
        json!({"x": 1}),
    )]));
    assert_eq!(
        capture_error(&history, std::slice::from_ref(&binding)),
        "checkpoint binding has ambiguous history call"
    );
    let repeated = call_history(&[call.clone(), call.clone(), call]);
    assert_eq!(
        capture_error(&repeated, std::slice::from_ref(&binding)),
        "checkpoint binding has ambiguous history call"
    );

    let unrelated = ToolCall::new("unbound", "probe", json!({}));
    let mut unrelated_history = call_history(&[unrelated.clone(), unrelated]);
    unrelated_history.push(CanonicalMessage::new(
        MessageRole::Tool,
        vec![ContentPart::ToolResult {
            result: ToolResult::ok("unbound".into(), "not selected"),
        }],
    ));
    unrelated_history.extend(call_history(std::slice::from_ref(&binding.execution_call)));
    assert!(HistoryCapture::new(&unrelated_history, &[binding]).is_ok());
    assert!(HistoryCapture::new(&history, &[]).is_ok());
}

#[test]
fn binding_validation_keeps_precedence_and_reserved_identity_checks() {
    let calls = [
        ToolCall::new("a", "probe", json!({})),
        ToolCall::new("b", "probe", json!({})),
    ];
    let history = call_history(&calls);
    let bindings = calls.map(WorkflowToolResultBinding::new);
    let mut changed = bindings[0].clone();
    changed.execution_call.id = "different".into();
    changed.message_id = history[0].id;
    assert_eq!(
        capture_error(&history, &[changed]),
        "checkpoint execution call id disagrees with history binding"
    );
    assert_eq!(
        capture_error(&history, &[bindings[0].clone(), bindings[0].clone()]),
        "duplicate checkpoint tool binding a"
    );
    for collide_message in [false, true] {
        let mut changed = bindings.clone();
        if collide_message {
            changed[1].message_id = changed[0].message_id;
        } else {
            changed[1].part_id = changed[0].part_id;
        }
        assert_eq!(
            capture_error(&history, &changed),
            "checkpoint result identities are not unique"
        );
        let mut changed = bindings[0].clone();
        if collide_message {
            changed.message_id = history[0].id;
        } else {
            changed.part_id = history[0].parts[0].part_id;
        }
        // Identity validation precedes even a missing history call.
        changed.call_id = "missing".into();
        changed.execution_call.id = changed.call_id.clone();
        assert_eq!(
            capture_error(&history, &[changed]),
            "checkpoint result identities are not unique"
        );
    }
    let missing = WorkflowToolResultBinding::new(ToolCall::new("missing", "probe", json!({})));
    assert_eq!(
        capture_error(
            &history,
            &[missing, bindings[0].clone(), bindings[0].clone()]
        ),
        "checkpoint binding missing has no history call"
    );
}

#[test]
fn execution_binding_is_exact_but_does_not_replace_the_model_call() {
    let model_call = ToolCall::new("a", "shell", json!({"command": "apply_patch"}));
    let execution_call = ToolCall::new("a", "apply_patch", json!({"patch": "..."}))
        .with_raw_arguments("original execution arguments");
    let history = call_history(std::slice::from_ref(&model_call));
    let binding = WorkflowToolResultBinding::new(execution_call.clone());
    let capture = HistoryCapture::new(&history, std::slice::from_ref(&binding)).unwrap();
    capture.validate_call(&execution_call).unwrap();
    assert!(capture.validate_call(&model_call).is_err());
    // Reused call IDs still require the complete binding, including execution
    // payload and both reserved IDs, even on either early-return path.
    let mut changes = vec![binding.clone(); 3];
    changes[0].execution_call.raw_arguments = None;
    changes[1].message_id = crate::domain::new_message_id();
    changes[2].part_id = crate::domain::new_part_id();
    for changed in changes {
        for compacted in [false, true] {
            assert_eq!(
                capture
                    .rebase(
                        &history,
                        &history,
                        std::slice::from_ref(&changed),
                        compacted
                    )
                    .unwrap_err()
                    .to_string(),
                "checkpoint changed a pending tool result binding"
            );
        }
    }
}

#[test]
fn rebase_preserves_partial_ack_and_reordered_pending_bindings() {
    let calls = [
        ToolCall::new("a", "probe", json!({})),
        ToolCall::new("b", "probe", json!({})),
        ToolCall::new("c", "probe", json!({})),
    ];
    let prefix = call_history(&calls);
    let bindings = calls.map(WorkflowToolResultBinding::new);
    let mut capture = HistoryCapture::new(&prefix, &bindings).unwrap();
    let mut current = prefix.clone();
    for id in ["c", "a", "b"] {
        capture
            .record(
                &mut current,
                &ToolResult::ok(id.into(), format!("result {id}")),
            )
            .unwrap();
    }
    let mut proposed = prefix;
    proposed.push(current[1].clone());
    proposed.push(CanonicalMessage::text(
        MessageRole::Assistant,
        "more model output",
    ));
    let pending = [bindings[2].clone(), bindings[1].clone()];
    let (_, rebased) = capture
        .rebase(&current, &proposed, &pending, false)
        .unwrap();
    assert_eq!(&rebased[..proposed.len()], &proposed);
    assert_eq!(
        &rebased[proposed.len()..],
        &[current[3].clone(), current[2].clone()]
    );
}

#[test]
fn rebase_message_index_keeps_the_first_equal_id_not_the_last() {
    let call = ToolCall::new("a", "probe", json!({}));
    let prefix = call_history(std::slice::from_ref(&call));
    let binding = WorkflowToolResultBinding::new(call);
    let mut capture = HistoryCapture::new(&prefix, &[binding]).unwrap();
    let mut current = prefix.clone();
    capture
        .record(&mut current, &ToolResult::ok("a".into(), "exact result"))
        .unwrap();
    let exact = current[1].clone();
    let mut changed = exact.clone();
    changed.parts[0].payload = ContentPart::Text {
        text: "changed".into(),
    };
    let mut proposed = prefix;
    // Force suffix rebasing rather than the starts_with(current) fast path.
    proposed.push(CanonicalMessage::text(
        MessageRole::Assistant,
        "new prefix item",
    ));
    proposed.extend([exact.clone(), changed.clone()]);
    let (_, rebased) = capture.rebase(&current, &proposed, &[], false).unwrap();
    assert_eq!(rebased, proposed);
    let len = proposed.len();
    proposed[len - 2] = changed;
    proposed[len - 1] = exact;
    assert_eq!(
        capture
            .rebase(&current, &proposed, &[], false)
            .unwrap_err()
            .to_string(),
        "checkpoint changed a committed tool result"
    );
}

#[test]
fn rebasing_preserves_exact_results_and_bindings_in_call_order() {
    let calls = [
        ToolCall::new("a", "probe", json!({})),
        ToolCall::new("b", "probe", json!({})),
    ];
    let mut prefix = vec![CanonicalMessage::new(
        MessageRole::Assistant,
        calls
            .iter()
            .cloned()
            .map(|call| ContentPart::ToolCall { call })
            .collect(),
    )];
    let bindings = calls
        .into_iter()
        .map(WorkflowToolResultBinding::new)
        .collect::<Vec<_>>();
    let mut capture = HistoryCapture::new(&prefix, &bindings).unwrap();
    let mut history = prefix.clone();
    let results = [
        ToolResult::ok("a".into(), "first"),
        ToolResult::ok("b".into(), "second"),
    ];
    capture.record(&mut history, &results[1]).unwrap();
    capture.record(&mut history, &results[0]).unwrap();
    prefix.push(CanonicalMessage::text(
        MessageRole::Assistant,
        "later model item",
    ));
    let (capture, rebased) = capture.rebase(&history, &prefix, &bindings, false).unwrap();
    assert_eq!(&rebased[..prefix.len()], &prefix);
    assert_eq!(
        &rebased[prefix.len()..],
        &[
            bindings[0].message(results[0].clone()),
            bindings[1].message(results[1].clone())
        ]
    );
    assert!(
        capture
            .rebase(&rebased, &prefix, &bindings[..1], false)
            .is_err()
    );
    let mut changed = bindings.clone();
    changed[0].part_id = crate::domain::new_part_id();
    assert!(capture.rebase(&rebased, &prefix, &changed, false).is_err());
    let (_, drained) = capture.rebase(&rebased, &rebased, &[], false).unwrap();
    assert_eq!(drained, rebased);
    assert!(capture.rebase(&rebased, &[], &[], false).is_err());
}
