use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use futures_util::stream;

use crate::{
    contracts::{
        ApprovalRequest, ApprovalResponse, ApprovalTransport, ContextBuildInput, ContextBuilder,
        ContextPreparation, ContextPreparationInput, ContextPreparationResult,
        ExecutionAttribution, Model, ModelEventStream, OperationOwner, Tool, ToolContext,
        ToolExecutionRecorder, ToolSource,
    },
    core::ModelResponseOutcome,
    domain::{ContextBundle, ModelRef, ToolCall, ToolCallResolution, ToolResult, ToolSpec},
    model_standard::{CanonicalModelRequest, ModelCapabilities, ModelStreamEvent},
};

use super::ReplayState;

pub(in crate::core::workflow_replay) struct ReplayModel {
    state: Arc<ReplayState>,
}

impl ReplayModel {
    pub fn new(state: Arc<ReplayState>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Model for ReplayModel {
    fn id(&self) -> std::borrow::Cow<'static, str> {
        "workflow-replay".into()
    }

    fn capabilities(&self, _model: &ModelRef) -> anyhow::Result<ModelCapabilities> {
        Ok(self.state.capabilities())
    }

    async fn stream(&self, request: CanonicalModelRequest) -> Result<ModelEventStream> {
        let (messages, outcome) = self.state.consume_model_request(&request)?;
        let event = match outcome {
            ModelResponseOutcome::Response { response } => ModelStreamEvent::Response { response },
            ModelResponseOutcome::Error { failure } => ModelStreamEvent::Error { failure },
        };
        let events = messages
            .into_iter()
            .map(|message| Ok(ModelStreamEvent::MessageCompleted { message }))
            .chain(std::iter::once(Ok(event)));
        Ok(Box::pin(stream::iter(events)))
    }
}

pub(in crate::core::workflow_replay) struct ReplayContextBuilder {
    state: Arc<ReplayState>,
}

impl ReplayContextBuilder {
    pub fn new(state: Arc<ReplayState>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl ContextBuilder for ReplayContextBuilder {
    async fn build(&self, _input: ContextBuildInput) -> Result<ContextBundle> {
        self.state.context()
    }
}

pub(in crate::core::workflow_replay) struct ReplayContextPreparation {
    state: Arc<ReplayState>,
    module_ids: Vec<String>,
}

impl ReplayContextPreparation {
    pub fn new(state: Arc<ReplayState>, module_ids: Vec<String>) -> Self {
        Self { state, module_ids }
    }
}

#[async_trait]
impl ContextPreparation for ReplayContextPreparation {
    async fn prepare(&self, input: ContextPreparationInput) -> Result<ContextPreparationResult> {
        self.state.prepare(input, self.module_ids.clone()).await
    }
}

struct ReplayTool {
    spec: ToolSpec,
    model_visible: bool,
    state: Arc<ReplayState>,
}

impl ReplayTool {
    fn new(spec: ToolSpec, model_visible: bool, state: Arc<ReplayState>) -> Self {
        Self {
            spec,
            model_visible,
            state,
        }
    }
}

#[async_trait]
impl Tool for ReplayTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn model_visible(&self) -> bool {
        self.model_visible
    }

    async fn invoke(&self, call: &ToolCall, _ctx: ToolContext) -> Result<ToolResult> {
        self.state.replay_tool_result(&call.id)
    }
}

pub(in crate::core::workflow_replay) struct ReplayApprovalTransport {
    state: Arc<ReplayState>,
    available: bool,
}

impl ReplayApprovalTransport {
    pub fn new(state: Arc<ReplayState>, available: bool) -> Self {
        Self { state, available }
    }
}

#[async_trait]
impl ApprovalTransport for ReplayApprovalTransport {
    fn can_request_approval(&self) -> bool {
        self.available
    }

    async fn request_approval(&self, request: ApprovalRequest) -> Result<ApprovalResponse> {
        self.state.approval_response(&request.call.id)
    }
}

#[async_trait]
impl ToolExecutionRecorder for ReplayState {
    async fn tool_call_requested(
        &self,
        _attribution: ExecutionAttribution,
        owner: OperationOwner,
        call: &ToolCall,
    ) -> Result<()> {
        let mut inner = self.lock();
        let index = match super::match_tool_index(&inner, call) {
            Ok(index) => index,
            Err(error) => return super::mismatch(&mut inner, error.to_string()),
        };
        if !super::operations::owners_equal(
            &owner,
            &inner.tools[index].recorded.owner,
            &inner.actual_to_expected,
        ) {
            return super::mismatch(
                &mut inner,
                "tool request owner differs from recorded owner".into(),
            );
        }
        drop(inner);
        self.record_tool_requested(call)
    }

    async fn tool_call_resolved(
        &self,
        _attribution: ExecutionAttribution,
        call: &ToolCall,
        resolution: &ToolCallResolution,
    ) -> Result<()> {
        self.record_tool_resolved(call, resolution)
    }

    async fn tool_approval_requested(
        &self,
        _attribution: ExecutionAttribution,
        call: &ToolCall,
        reason: &str,
    ) -> Result<()> {
        self.record_approval_requested(call, reason)
    }

    async fn tool_result_recorded(
        &self,
        _attribution: ExecutionAttribution,
        result: &ToolResult,
    ) -> Result<()> {
        self.record_tool_result(result)
    }
}

pub(in crate::core::workflow_replay) fn register_replay_tools(
    state: Arc<ReplayState>,
    specs: impl IntoIterator<Item = (ToolSpec, bool)>,
) -> Result<crate::contracts::ToolRegistry> {
    let mut registry = crate::contracts::ToolRegistry::new();
    for (spec, model_visible) in specs {
        registry.register_with_source(
            ToolSource::Dynamic {
                origin: "workflow_replay".to_owned(),
            },
            ReplayTool::new(spec, model_visible, state.clone()),
        )?;
    }
    Ok(registry)
}
