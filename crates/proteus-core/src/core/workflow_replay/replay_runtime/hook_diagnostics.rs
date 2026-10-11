//! Field-level mismatch evidence without printing request material or secrets.
use super::*;
pub(super) fn preparation_difference(
    actual: &HookEvent,
    expected: &HookEvent,
    ids: &HashMap<CallId, CallId>,
) -> String {
    let (
        HookEvent::BeforeContext {
            input: a,
            output: ao,
            target: at,
        },
        HookEvent::BeforeContext {
            input: b,
            output: bo,
            target: bt,
        },
    ) = (actual, expected)
    else {
        return "event kind differs".into();
    };
    let mut fields = Vec::new();
    if a.reason != b.reason {
        fields.push(format!(
            "input.reason actual={:?} recorded={:?}",
            a.reason, b.reason
        ));
    }
    if a.token_estimate != b.token_estimate {
        fields.push(format!(
            "input.token_estimate actual={:?} recorded={:?}",
            a.token_estimate, b.token_estimate
        ));
    }
    if a.window_tokens != b.window_tokens {
        fields.push(format!(
            "input.window_tokens actual={:?} recorded={:?}",
            a.window_tokens, b.window_tokens
        ));
    }
    if !messages_equal(&a.messages, &b.messages, ids) {
        fields.push("input.messages".into());
    }
    match (&a.request, &b.request) {
        (Some(a), Some(b)) if !requests_equal(a, b, ids) => fields.push(format!(
            "input.request {}",
            super::super::super::normalize::request_difference(a, b, ids)
        )),
        (None, Some(_)) | (Some(_), None) => fields.push("input.request presence".into()),
        _ => {}
    }
    if !messages_equal(&ao.messages, &bo.messages, ids) {
        fields.push("output.messages".into());
    }
    let mut a = ao.clone();
    let mut b = bo.clone();
    a.messages.clear();
    b.messages.clear();
    if a != b {
        fields.push("output diagnostics".into());
    }
    if at != bt {
        fields.push(format!("target actual={at:?} recorded={bt:?}"));
    }
    fields.join("; ")
}
