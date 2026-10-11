use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
};

use anyhow::{Result, anyhow, bail};

use crate::{
    contracts::{ApprovalResponse, OperationOwner},
    core::ModelResponseOutcome,
    domain::{
        CacheHints, CallId, ContextBundle, HistoryCompactionReport, HostedToolKind, MessageId,
        ReasoningConfig, ToolCall, ToolCallResolution, ToolResult, ToolSurface,
    },
    model_standard::{CanonicalModelRequest, ModelCapabilities},
};

use super::{
    fixture::{RecordedModelExchange, RecordedToolInvocation},
    normalize::{
        calls_equal, messages_equal, outputs_equal, request_difference, requests_equal,
        results_equal, rewrite_result_call_ids,
    },
};

mod adapters;
mod hooks;
pub(super) use hooks::ReplayHooks;
mod checkpoints;
mod operations;
mod preparation;
pub(super) use checkpoints::{RecordedCheckpoint, ReplayCheckpointRecorder, recorded_checkpoints};

pub(super) use adapters::{
    ReplayApprovalTransport, ReplayContextBuilder, ReplayContextPreparation, ReplayModel,
    register_replay_tools,
};

pub(super) struct ReplayState {
    inner: Mutex<ReplayStateInner>,
    capabilities: ModelCapabilities,
    context: Option<ContextBundle>,
}

struct ReplayStateInner {
    hooks: Vec<(crate::contracts::HookTrace, bool)>,
    exchanges: Vec<RecordedModelExchange>,
    next_exchange: usize,
    auxiliary: Vec<(RecordedModelExchange, bool)>,
    target: Option<crate::contracts::ContextPreparationTarget>,
    message_ids: HashMap<crate::domain::MessageId, crate::domain::MessageId>,
    part_ids: HashMap<crate::domain::PartId, crate::domain::PartId>,
    tools: Vec<ExpectedToolState>,
    actual_to_expected: HashMap<CallId, CallId>,
    expected_to_actual: HashMap<CallId, CallId>,
    compactions: Vec<ExpectedCompaction>,
    issues: Vec<String>,
}

struct ExpectedToolState {
    recorded: RecordedToolInvocation,
    requested: bool,
    approval_requested: bool,
    resolved: bool,
    result_recorded: bool,
}

struct ExpectedCompaction {
    report: HistoryCompactionReport,
    consumed: bool,
}

pub(super) struct ReplaySummary {
    pub model_exchanges: usize,
    pub tool_calls: usize,
    pub issues: Vec<String>,
}

impl ReplayState {
    pub fn new(
        exchanges: Vec<RecordedModelExchange>,
        hooks: Vec<crate::contracts::HookTrace>,
        tools: Vec<RecordedToolInvocation>,
        compactions: Vec<HistoryCompactionReport>,
        context: Option<ContextBundle>,
        snapshot_reasoning: &ReasoningConfig,
        _incoming_user_message_id: MessageId,
    ) -> Self {
        let capabilities = exchanges
            .first()
            .map(|exchange| replay_capabilities(&exchange.request, snapshot_reasoning))
            .or_else(|| {
                hooks.iter().find_map(|trace| match &trace.input.event {
                    crate::contracts::HookEvent::BeforeModel { request, .. } => {
                        Some(replay_capabilities(request, snapshot_reasoning))
                    }
                    _ => None,
                })
            })
            .unwrap_or_else(ModelCapabilities::empty);
        Self {
            inner: Mutex::new(ReplayStateInner {
                hooks: hooks.into_iter().map(|trace| (trace, false)).collect(),
                exchanges,
                next_exchange: 0,
                auxiliary: Vec::new(),
                target: None,
                message_ids: HashMap::new(),
                part_ids: HashMap::new(),
                tools: tools
                    .into_iter()
                    .map(|recorded| ExpectedToolState {
                        recorded,
                        requested: false,
                        approval_requested: false,
                        resolved: false,
                        result_recorded: false,
                    })
                    .collect(),
                actual_to_expected: HashMap::new(),
                expected_to_actual: HashMap::new(),
                compactions: compactions
                    .into_iter()
                    .map(|report| ExpectedCompaction {
                        report,
                        consumed: false,
                    })
                    .collect(),
                issues: Vec::new(),
            }),
            capabilities,
            context,
        }
    }

    pub fn bind_history(&self, execution_id: crate::domain::ExecutionId, revision: u64) {
        self.lock().target = Some(crate::contracts::ContextPreparationTarget {
            execution_id,
            revision,
        });
    }

    pub fn set_auxiliary(&self, exchanges: Vec<RecordedModelExchange>) {
        self.lock().auxiliary = exchanges
            .into_iter()
            .map(|exchange| (exchange, false))
            .collect();
    }

    pub fn capabilities(&self) -> ModelCapabilities {
        self.capabilities.clone()
    }

    pub fn context(&self) -> Result<ContextBundle> {
        // Each reviewed candidate belongs to its own context-build invocation;
        // later candidates may include fresh workspace search results.
        let candidate = self.lock().hooks.iter().find_map(|(trace, consumed)| {
            if *consumed {
                return None;
            }
            match &trace.input.event {
                crate::contracts::HookEvent::BeforeStop { output, .. } => Some(output.clone()),
                _ => None,
            }
        });
        if let Some(output) = candidate {
            return Ok(super::fixture::recorded_context(
                &self.current_request()?,
                Some(&output),
            ));
        }
        match &self.context {
            Some(context) => Ok(context.clone()),
            None => mismatch(
                &mut self.lock(),
                "workflow requested context without a recorded model request; the journal has no replayable context bundle".to_owned(),
            ),
        }
    }

    pub fn current_request(&self) -> Result<CanonicalModelRequest> {
        let mut inner = self.lock();
        if let Some(request) = inner.hooks.iter().find_map(|(trace, consumed)| {
            if *consumed {
                return None;
            }
            match &trace.input.event {
                crate::contracts::HookEvent::BeforeModel {
                    origin: crate::contracts::ModelCallOrigin::Direct,
                    request,
                } => Some(request.clone()),
                _ => None,
            }
        }) {
            return Ok(request);
        }
        // Failed preparation may have no following direct model request. Its
        // template still proves the selected tools. Prefer BeforeModel above:
        // workflow adds selector metadata only after preparation completes.
        if let Some(request) = inner.hooks.iter().find_map(|(trace, consumed)| {
            if *consumed || trace.owner != OperationOwner::Workflow {
                return None;
            }
            match &trace.input.event {
                crate::contracts::HookEvent::BeforeContext { input, .. } => input.request.clone(),
                _ => None,
            }
        }) {
            return Ok(request);
        }
        match inner.exchanges.get(inner.next_exchange) {
            Some(exchange) => Ok(exchange.request.clone()),
            None => mismatch(
                &mut inner,
                "workflow requested context without a remaining recorded model request".to_owned(),
            ),
        }
    }

    pub fn consume_model_request(
        &self,
        actual: &CanonicalModelRequest,
    ) -> Result<(
        Vec<crate::model_standard::CanonicalMessage>,
        ModelResponseOutcome,
    )> {
        let mut inner = self.lock();
        let index = inner.next_exchange;
        let Some(expected) = inner.exchanges.get(index).cloned() else {
            return mismatch(
                &mut inner,
                format!(
                    "workflow emitted unexpected model request #{} after the recorded exchange script ended",
                    index + 1
                ),
            );
        };
        if !requests_equal(actual, &expected.request, &inner.actual_to_expected) {
            let difference =
                request_difference(actual, &expected.request, &inner.actual_to_expected);
            return mismatch(
                &mut inner,
                format!(
                    "model request #{} does not match recorded exchange {} ({difference})",
                    index + 1,
                    expected.exchange_id
                ),
            );
        }
        let outcome = expected.outcome.clone();
        inner.next_exchange += 1;
        Ok((expected.completed_messages, outcome))
    }

    pub fn approval_response(&self, actual_call_id: &str) -> Result<ApprovalResponse> {
        let inner = self.lock();
        let expected = expected_tool(&inner, actual_call_id)?;
        match &expected.recorded.resolution {
            ToolCallResolution::Approved => Ok(ApprovalResponse::approve()),
            ToolCallResolution::ApprovalDenied { reason } => {
                Ok(ApprovalResponse::deny(reason.clone()))
            }
            resolution => bail!(
                "workflow requested replay approval for tool call {}, but recorded resolution is {resolution:?}",
                expected.recorded.call.id
            ),
        }
    }

    pub fn replay_tool_result(&self, actual_call_id: &str) -> Result<ToolResult> {
        let mut inner = self.lock();
        let expected_id = expected_tool(&inner, actual_call_id)?
            .recorded
            .call
            .id
            .clone();
        operations::consume_body(
            &mut inner,
            &OperationOwner::Tool {
                call_id: expected_id,
            },
        );
        let expected = expected_tool(&inner, actual_call_id)?;
        if !expected.recorded.resolution.permits_side_effect() {
            bail!(
                "workflow tried to invoke replay tool {}, but recorded resolution {:?} did not permit invocation",
                expected.recorded.call.id,
                expected.recorded.resolution
            );
        }
        let mut result = expected
            .recorded
            .raw_result
            .clone()
            .or_else(|| {
                inner
                    .hooks
                    .iter()
                    .find_map(|(trace, _)| match &trace.input.event {
                        crate::contracts::HookEvent::AfterTool { call, result }
                            if call.id == expected.recorded.call.id =>
                        {
                            Some(result.clone())
                        }
                        _ => None,
                    })
            })
            .unwrap_or_else(|| expected.recorded.result.clone());
        rewrite_result_call_ids(&mut result, &inner.expected_to_actual);
        result.call_id = actual_call_id.to_owned();
        Ok(result)
    }

    pub fn summary(&self) -> ReplaySummary {
        let mut inner = self.lock();
        let remaining_hooks = inner.hooks.iter().filter(|(_, consumed)| !consumed).count();
        if remaining_hooks != 0 {
            inner.issues.push(format!(
                "workflow left {remaining_hooks} recorded hook boundaries unconsumed"
            ));
        }
        let remaining_auxiliary = inner
            .auxiliary
            .iter()
            .filter(|(_, consumed)| !consumed)
            .count();
        if remaining_auxiliary != 0 {
            inner.issues.push(format!(
                "workflow left {remaining_auxiliary} owned auxiliary exchanges unconsumed"
            ));
        }
        let model_exchanges = inner.next_exchange;
        let tool_calls = inner.tools.iter().filter(|tool| tool.requested).count();
        if model_exchanges != inner.exchanges.len() {
            let total = inner.exchanges.len();
            inner.issues.push(format!(
                "workflow consumed {model_exchanges} of {total} recorded model exchanges"
            ));
        }
        let missing = inner
            .tools
            .iter()
            .filter(|tool| {
                !tool.requested
                    || !tool.resolved
                    || !tool.result_recorded
                    || (tool.recorded.approval_reason.is_some() && !tool.approval_requested)
            })
            .map(|tool| tool.recorded.call.id.clone())
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            inner.issues.push(format!(
                "workflow did not consume complete lifecycle for recorded tool calls: {}",
                missing.join(", ")
            ));
        }
        let unconsumed_compactions = inner
            .compactions
            .iter()
            .filter(|compaction| !compaction.consumed)
            .count();
        if unconsumed_compactions > 0 {
            inner.issues.push(format!(
                "workflow did not reproduce {unconsumed_compactions} recorded changed compaction(s)"
            ));
        }
        ReplaySummary {
            model_exchanges,
            tool_calls,
            issues: inner.issues.clone(),
        }
    }

    pub fn histories_equal(
        &self,
        actual: &[crate::model_standard::CanonicalMessage],
        expected: &[crate::model_standard::CanonicalMessage],
    ) -> bool {
        let inner = self.lock();
        messages_equal(actual, expected, &inner.actual_to_expected)
    }

    pub fn outputs_equal(
        &self,
        actual: &crate::domain::AgentOutput,
        expected: &crate::domain::AgentOutput,
    ) -> bool {
        let inner = self.lock();
        outputs_equal(actual, expected, &inner.actual_to_expected)
    }

    fn record_tool_requested(&self, call: &ToolCall) -> Result<()> {
        let mut inner = self.lock();
        let index = match match_tool_index(&inner, call) {
            Ok(index) => index,
            Err(error) => return mismatch(&mut inner, error.to_string()),
        };
        if inner.tools[index].requested {
            return mismatch(
                &mut inner,
                format!(
                    "tool call {} was requested more than once during replay",
                    call.id
                ),
            );
        }
        let expected_id = inner.tools[index].recorded.call.id.clone();
        inner
            .actual_to_expected
            .insert(call.id.clone(), expected_id.clone());
        inner
            .expected_to_actual
            .insert(expected_id.clone(), call.id.clone());
        if !calls_equal(
            call,
            &inner.tools[index].recorded.call,
            &inner.actual_to_expected,
        ) {
            return mismatch(
                &mut inner,
                format!(
                    "tool call {} does not match recorded call {expected_id}",
                    call.id
                ),
            );
        }
        inner.tools[index].requested = true;
        Ok(())
    }

    fn record_approval_requested(&self, call: &ToolCall, reason: &str) -> Result<()> {
        let mut inner = self.lock();
        let index = mapped_tool_index(&inner, &call.id)?;
        let expected_reason = inner.tools[index].recorded.approval_reason.clone();
        let expected_call_id = inner.tools[index].recorded.call.id.clone();
        if expected_reason.as_deref() != Some(reason) {
            return mismatch(
                &mut inner,
                format!(
                    "approval reason for tool call {} differs: recorded={expected_reason:?}, replay={reason:?}",
                    expected_call_id
                ),
            );
        }
        inner.tools[index].approval_requested = true;
        Ok(())
    }

    fn record_tool_resolved(&self, call: &ToolCall, resolution: &ToolCallResolution) -> Result<()> {
        let mut inner = self.lock();
        let index = mapped_tool_index(&inner, &call.id)?;
        let expected_call_id = inner.tools[index].recorded.call.id.clone();
        let expected_resolution = inner.tools[index].recorded.resolution.clone();
        if expected_resolution != *resolution {
            return mismatch(
                &mut inner,
                format!(
                    "tool resolution for {} differs: recorded={:?}, replay={resolution:?}",
                    expected_call_id, expected_resolution
                ),
            );
        }
        inner.tools[index].resolved = true;
        Ok(())
    }

    fn record_tool_result(&self, result: &ToolResult) -> Result<()> {
        let mut inner = self.lock();
        let index = mapped_tool_index(&inner, &result.call_id)?;
        let expected_call_id = inner.tools[index].recorded.call.id.clone();
        let expected_result = inner.tools[index].recorded.result.clone();
        if !results_equal(result, &expected_result, &inner.actual_to_expected) {
            return mismatch(
                &mut inner,
                format!(
                    "tool result for {} differs from the recorded result",
                    expected_call_id
                ),
            );
        }
        inner.tools[index].result_recorded = true;
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, ReplayStateInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn replay_capabilities(
    request: &CanonicalModelRequest,
    snapshot_reasoning: &ReasoningConfig,
) -> ModelCapabilities {
    let hosted = request
        .tools
        .iter()
        .filter_map(|tool| match &tool.surface {
            ToolSurface::ProviderHosted { config } => Some(config.kind()),
            _ => None,
        })
        .collect::<Vec<HostedToolKind>>();
    ModelCapabilities::empty()
        .with_tools(true)
        // Compare media with the recorded request; replay never encodes or
        // sends image bytes to a provider, including later steering inputs.
        .with_image_input(true)
        .with_parallel_tool_calls(true)
        .with_freeform_tools(
            request
                .tools
                .iter()
                .any(|tool| matches!(tool.surface, ToolSurface::Freeform { .. })),
        )
        .with_streaming(true)
        .with_json_schema(true)
        .with_system_role(true)
        .with_developer_role(true)
        .with_cache_hints(request.cache != CacheHints::default())
        .with_reasoning_config(
            request.reasoning != ReasoningConfig::default()
                || *snapshot_reasoning == ReasoningConfig::default(),
        )
        .with_provider_hosted_tools(hosted)
        .with_max_input_tokens(request.limits.max_input_tokens)
        .with_max_output_tokens(request.limits.max_output_tokens)
}

fn match_tool_index(inner: &ReplayStateInner, actual: &ToolCall) -> Result<usize> {
    // Checkpoints may bind identical calls before their concurrent dispatch.
    // Preserve that bijection even when workers reach the host out of order.
    if let Some(expected_id) = inner.actual_to_expected.get(&actual.id) {
        return inner
            .tools
            .iter()
            .position(|tool| &tool.recorded.call.id == expected_id)
            .ok_or_else(|| anyhow!("checkpoint-bound replay tool {expected_id} is missing"));
    }
    if let Some(index) = inner
        .tools
        .iter()
        .position(|tool| !tool.requested && tool.recorded.call.id == actual.id)
    {
        return Ok(index);
    }
    inner
        .tools
        .iter()
        .position(|tool| {
            !tool.requested
                && tool.recorded.call.name == actual.name
                && tool.recorded.call.args == actual.args
                && tool.recorded.call.surface == actual.surface
                && tool.recorded.call.raw_arguments == actual.raw_arguments
        })
        .ok_or_else(|| {
            anyhow!(
                "unexpected replay tool call {} ({})",
                actual.id,
                actual.name
            )
        })
}

fn mapped_tool_index(inner: &ReplayStateInner, actual_call_id: &str) -> Result<usize> {
    let expected_id = inner
        .actual_to_expected
        .get(actual_call_id)
        .ok_or_else(|| anyhow!("replay tool call {actual_call_id} was not requested"))?;
    inner
        .tools
        .iter()
        .position(|tool| tool.recorded.call.id == *expected_id)
        .ok_or_else(|| anyhow!("recorded tool call {expected_id} is missing from replay state"))
}

fn expected_tool<'a>(
    inner: &'a ReplayStateInner,
    actual_call_id: &str,
) -> Result<&'a ExpectedToolState> {
    let index = mapped_tool_index(inner, actual_call_id)?;
    inner
        .tools
        .get(index)
        .ok_or_else(|| anyhow!("replay tool index {index} is invalid"))
}

fn mismatch<T>(inner: &mut ReplayStateInner, message: String) -> Result<T> {
    inner.issues.push(message.clone());
    bail!(message)
}
