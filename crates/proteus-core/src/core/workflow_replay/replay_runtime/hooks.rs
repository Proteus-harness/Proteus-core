//! Recorded hook outcomes are replay inputs; no hook executable is launched.
use super::super::normalize::{
    calls_equal, messages_equal, outputs_equal, requests_equal, results_equal,
};
use super::{ReplayState, mismatch};
use crate::{
    contracts::{ExecutionHooks, HookEvent, HookInput, HookResponse, HookStepOutcome},
    core::session_journal::apply_recorded_hook_response,
    domain::CallId,
    model_standard::RequestShaper,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::{collections::HashMap, sync::Arc};
#[path = "hook_diagnostics.rs"]
mod diagnostics;

pub(in crate::core::workflow_replay) struct ReplayHooks {
    state: Arc<ReplayState>,
    module_ids: Vec<String>,
}
impl ReplayHooks {
    pub fn new(state: Arc<ReplayState>, module_ids: Vec<String>) -> Self {
        Self { state, module_ids }
    }
}

#[async_trait]
impl ExecutionHooks for ReplayHooks {
    fn is_active(&self) -> bool {
        !self.module_ids.is_empty()
    }
    async fn apply(&self, input: HookInput) -> Result<HookEvent> {
        if self.module_ids.is_empty()
            && !matches!(&input.event, HookEvent::BeforeToolSelection { .. })
        {
            let mut event = input.event;
            if let HookEvent::AfterTool { call, result } = &mut event {
                let inner = self.state.lock();
                let recorded = super::expected_tool(&inner, &call.id)?;
                let original = recorded
                    .recorded
                    .raw_result
                    .as_ref()
                    .unwrap_or(&recorded.recorded.result);
                restore_duration(result, original);
            }
            return Ok(event);
        }
        let mut inner = self.state.lock();
        let candidate = inner.hooks.iter().position(|(trace, consumed)| {
            !consumed
                && super::operations::owners_equal(
                    &crate::core::model_call_scope::current_operation_owner(),
                    &trace.owner,
                    &inner.actual_to_expected,
                )
                && same_boundary(&input.event, &trace.input.event, &inner.actual_to_expected)
        });
        let Some(index) = candidate else {
            return mismatch(
                &mut inner,
                format!(
                    "workflow emitted unexpected hook boundary {}",
                    boundary_name(&input.event)
                ),
            );
        };
        let trace = inner.hooks[index].0.clone();
        if trace.steps.len() > self.module_ids.len()
            || trace
                .steps
                .iter()
                .zip(&self.module_ids)
                .any(|(step, id)| &step.module_id != id)
        {
            return mismatch(
                &mut inner,
                "recorded hook contribution order differs from config snapshot".to_owned(),
            );
        }
        let stopped = trace.output.is_none()
            || matches!(
                &trace.output,
                Some(
                    HookEvent::BeforeTool {
                        blocked: Some(_),
                        ..
                    } | HookEvent::BeforeStop {
                        continuation: Some(_),
                        ..
                    }
                )
            );
        if !stopped && trace.steps.len() != self.module_ids.len() {
            return mismatch(
                &mut inner,
                "recorded successful hook boundary omitted a configured contribution".to_owned(),
            );
        }
        if input.cwd != trace.input.cwd
            || input.attribution.agent != trace.input.attribution.agent
            || !events_equal(&input.event, &trace.input.event, &inner.actual_to_expected)
        {
            let fields = match (&input.event, &trace.input.event) {
                (
                    HookEvent::BeforeModel { request: a, .. },
                    HookEvent::BeforeModel { request: b, .. },
                ) => super::super::normalize::request_difference(a, b, &inner.actual_to_expected),
                (HookEvent::BeforeContext { .. }, HookEvent::BeforeContext { .. }) => {
                    diagnostics::preparation_difference(
                        &input.event,
                        &trace.input.event,
                        &inner.actual_to_expected,
                    )
                }
                _ => String::new(),
            };
            return mismatch(
                &mut inner,
                format!(
                    "hook boundary {} input differs from recorded input; {}",
                    boundary_name(&input.event),
                    fields
                ),
            );
        }
        inner.hooks[index].1 = true;
        let mut event = input.event;
        // Invocation stubs measure a new duration. Feed the recorded timing
        // back into their oracle result before the workflow estimates its size.
        // All semantic result fields were compared above; only duration is
        // explicitly excluded by the replay normalization boundary.
        if let (
            HookEvent::AfterTool { result, .. },
            HookEvent::AfterTool {
                result: recorded, ..
            },
        ) = (&mut event, &trace.input.event)
        {
            restore_duration(result, recorded);
        }
        let mut recorded_event = trace.input.event.clone();
        for step in &trace.steps {
            super::operations::consume_hook(&mut inner, step.invocation_id);
            match &step.outcome {
                HookStepOutcome::Accepted { response } => {
                    let mut rewritten = rewrite_response(response, &inner.expected_to_actual)?;
                    rewrite_preparation_identities(&mut rewritten, &recorded_event, &event)?;
                    recorded_event = apply_recorded_hook_response(&recorded_event, response)?;
                    event = apply_recorded_hook_response(&event, &rewritten)?;
                    if let HookEvent::BeforeModel { request, .. } = &mut event {
                        *request =
                            RequestShaper.shape(request.clone(), &self.state.capabilities())?;
                    }
                }
                HookStepOutcome::Failed {
                    message,
                    model_failure,
                } if trace.output.is_none() => {
                    return Err(match model_failure {
                        Some(failure) if failure.message == *message => {
                            anyhow::Error::new(failure.clone())
                        }
                        Some(failure) => {
                            anyhow::Error::new(failure.clone()).context(message.clone())
                        }
                        None => anyhow!(message.clone()),
                    });
                }
                HookStepOutcome::Failed { .. } => {}
            }
        }
        let Some(expected) = &trace.output else {
            return mismatch(
                &mut inner,
                "recorded failed hook boundary has no failure step".to_owned(),
            );
        };
        if !events_equal(&event, expected, &inner.actual_to_expected) {
            return mismatch(
                &mut inner,
                format!(
                    "hook boundary {} replayed output differs from recorded output",
                    boundary_name(&event)
                ),
            );
        }
        Ok(event)
    }
}

fn restore_duration(actual: &mut crate::domain::ToolResult, recorded: &crate::domain::ToolResult) {
    if let Some(metadata) = actual.metadata.as_object_mut() {
        match recorded.metadata.get("duration_ms") {
            Some(duration) => {
                metadata.insert("duration_ms".into(), duration.clone());
            }
            None => {
                metadata.remove("duration_ms");
            }
        }
    }
}

fn same_boundary(actual: &HookEvent, expected: &HookEvent, ids: &HashMap<CallId, CallId>) -> bool {
    match (actual, expected) {
        (HookEvent::BeforeTool { call: a, .. }, HookEvent::BeforeTool { call: b, .. })
        | (HookEvent::AfterTool { call: a, .. }, HookEvent::AfterTool { call: b, .. }) => {
            ids.get(&a.id).unwrap_or(&a.id) == &b.id
        }
        (HookEvent::BeforeModel { origin: a, .. }, HookEvent::BeforeModel { origin: b, .. }) => {
            a == b
        }
        (HookEvent::BeforeStop { attempt: a, .. }, HookEvent::BeforeStop { attempt: b, .. }) => {
            a == b
        }
        _ => std::mem::discriminant(actual) == std::mem::discriminant(expected),
    }
}

fn events_equal(actual: &HookEvent, expected: &HookEvent, ids: &HashMap<CallId, CallId>) -> bool {
    match (actual, expected) {
        (
            HookEvent::TurnStarted {
                task: a,
                history: ah,
            },
            HookEvent::TurnStarted {
                task: b,
                history: bh,
            },
        ) => a == b && messages_equal(ah, bh, ids),
        (
            HookEvent::BeforeToolSelection {
                input: a,
                output: ao,
            },
            HookEvent::BeforeToolSelection {
                input: b,
                output: bo,
            },
        ) => a == b && ao == bo,
        (
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
        ) => {
            a.reason == b.reason
                && a.token_estimate == b.token_estimate
                && a.window_tokens == b.window_tokens
                && messages_equal(&a.messages, &b.messages, ids)
                && match (&a.request, &b.request) {
                    (Some(a), Some(b)) => requests_equal(a, b, ids),
                    (None, None) => true,
                    _ => false,
                }
                && messages_equal(&ao.messages, &bo.messages, ids)
                && ao.changed == bo.changed
                && ao.summary == bo.summary
                && ao.token_estimate == bo.token_estimate
                && ao.original_token_estimate == bo.original_token_estimate
                && ao.trigger_tokens == bo.trigger_tokens
                && ao.summary_source == bo.summary_source
                && ao.skipped_reason == bo.skipped_reason
                && ao.metadata == bo.metadata
                && ao.user_message_replacements == bo.user_message_replacements
                && at == bt
        }
        (
            HookEvent::BeforeModel {
                origin: a,
                request: ar,
            },
            HookEvent::BeforeModel {
                origin: b,
                request: br,
            },
        ) => a == b && requests_equal(ar, br, ids),
        (
            HookEvent::BeforeTool {
                call: a,
                spec: aspec,
                blocked: ab,
            },
            HookEvent::BeforeTool {
                call: b,
                spec: bspec,
                blocked: bb,
            },
        ) => calls_equal(a, b, ids) && aspec == bspec && ab == bb,
        (
            HookEvent::AfterTool {
                call: a,
                result: ar,
            },
            HookEvent::AfterTool {
                call: b,
                result: br,
            },
        ) => calls_equal(a, b, ids) && results_equal(ar, br, ids),
        (
            HookEvent::BeforeStop {
                task: a,
                history: ah,
                output: ao,
                attempt: aa,
                continuation: ac,
            },
            HookEvent::BeforeStop {
                task: b,
                history: bh,
                output: bo,
                attempt: ba,
                continuation: bc,
            },
        ) => {
            a == b
                && messages_equal(ah, bh, ids)
                && outputs_equal(ao, bo, ids)
                && aa == ba
                && ac == bc
        }
        (
            HookEvent::TurnSettled {
                status: a,
                output: ao,
                error: ae,
            },
            HookEvent::TurnSettled {
                status: b,
                output: bo,
                error: be,
            },
        ) => {
            a == b
                && ae == be
                && match (ao, bo) {
                    (Some(a), Some(b)) => outputs_equal(a, b, ids),
                    (None, None) => true,
                    _ => false,
                }
        }
        _ => false,
    }
}

/// The recorded trace proves immutable source content. Rebind its retained
/// canonical identities to the current replay material; fresh oracle IDs remain.
fn rewrite_preparation_identities(
    response: &mut HookResponse,
    expected: &HookEvent,
    actual: &HookEvent,
) -> Result<()> {
    let HookResponse::ContextPreparation { output } = response else {
        return Ok(());
    };
    let (
        HookEvent::BeforeContext {
            input: expected_input,
            output: expected_output,
            ..
        },
        HookEvent::BeforeContext {
            input: actual_input,
            output: actual_output,
            ..
        },
    ) = (expected, actual)
    else {
        return Ok(());
    };
    let mut messages = HashMap::new();
    let mut parts = HashMap::new();
    for (source, current) in expected_input
        .messages
        .iter()
        .zip(&actual_input.messages)
        .chain(expected_output.messages.iter().zip(&actual_output.messages))
    {
        messages.insert(source.id, current);
        for (source_part, current_part) in source.parts.iter().zip(&current.parts) {
            parts.insert(source_part.part_id, current_part);
        }
    }
    for message in &mut output.messages {
        if let Some(current) = messages.get(&message.id) {
            *message = (*current).clone();
        } else {
            for part in &mut message.parts {
                if let Some(current) = parts.get(&part.part_id) {
                    *part = (*current).clone();
                }
            }
        }
    }
    for replacement in &mut output.user_message_replacements {
        if let Some(current) = messages.get(&replacement.source_message_id) {
            replacement.source_message_id = current.id;
        }
        if let Some(current) = messages.get(&replacement.replacement_message_id) {
            replacement.replacement_message_id = current.id;
        }
    }
    Ok(())
}

fn rewrite_response(
    response: &HookResponse,
    ids: &HashMap<CallId, CallId>,
) -> Result<HookResponse> {
    let mut value = serde_json::to_value(response)?;
    rewrite_strings(&mut value, ids);
    Ok(serde_json::from_value(value)?)
}
pub(super) fn rewrite_strings(value: &mut serde_json::Value, ids: &HashMap<CallId, CallId>) {
    match value {
        serde_json::Value::String(text) => {
            if let Some(actual) = ids.get(text) {
                *text = actual.clone();
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                rewrite_strings(value, ids);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                rewrite_strings(value, ids);
            }
        }
        _ => {}
    }
}
fn boundary_name(event: &HookEvent) -> &'static str {
    match event {
        HookEvent::TurnStarted { .. } => "turn_started",
        HookEvent::BeforeModel { .. } => "before_model",
        HookEvent::BeforeContext { .. } => "before_context",
        HookEvent::BeforeToolSelection { .. } => "before_tool_selection",
        HookEvent::BeforeTool { .. } => "before_tool",
        HookEvent::AfterTool { .. } => "after_tool",
        HookEvent::BeforeStop { .. } => "before_stop",
        HookEvent::TurnSettled { .. } => "turn_settled",
    }
}

#[cfg(test)]
#[path = "hook_tests.rs"]
mod tests;
