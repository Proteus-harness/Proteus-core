//! Exercise production execution binding, including the hook-owned auxiliary model.
use super::*;
use crate::{
    contracts::{
        ContextPreparation, ContextPreparationInput, ExecutionAttribution, ExecutionRecorder,
        ExecutionScope, HookEvent, HookHandler, HookHost, HookInput, HookResponse, HookTrace,
        ModelCallOrigin, ModelEventStream, OperationOwner,
    },
    core::{HeadlessApprovalTransport, context_preparation::RuntimeContextPreparation},
    domain::{ExchangeId, PermissionMode},
    model_standard::{
        CanonicalMessage, CanonicalModelRequest, CanonicalModelResponse, ContentPart, FinishReason,
        InstructionBlock, InstructionKind, MessageRole, ModelCapabilities, ModelOperation,
        ModelStreamEvent, TokenUsage,
    },
};
use async_trait::async_trait;
use std::sync::Mutex;

#[derive(Default)]
struct Provider(Mutex<Vec<CanonicalModelRequest>>);
#[async_trait]
impl Model for Provider {
    fn id(&self) -> std::borrow::Cow<'static, str> {
        "fixture".into()
    }
    fn capabilities(&self, _: &ModelRef) -> Result<ModelCapabilities> {
        Ok(ModelCapabilities::empty().with_compaction(true))
    }
    async fn estimate_input(
        &self,
        request: crate::contracts::ModelInputEstimateRequest,
    ) -> Result<Option<crate::contracts::ModelInputEstimate>> {
        assert_eq!(request.model, ModelRef::new("fixture", "native"));
        assert!(matches!(
            crate::core::model_call_scope::current_operation_owner(),
            OperationOwner::Hook { .. }
        ));
        Ok(Some(crate::contracts::ModelInputEstimate {
            parts: vec![crate::contracts::ModelPartTokenEstimate {
                part_id: request.parts[0].part_id,
                tokens: 13,
            }],
        }))
    }
    async fn stream(&self, request: CanonicalModelRequest) -> Result<ModelEventStream> {
        let operation = request.operation;
        self.0.lock().unwrap().push(request);
        let message = if operation == ModelOperation::Compact {
            CanonicalMessage::new(
                MessageRole::Assistant,
                vec![ContentPart::Opaque {
                    format: "org.example/checkpoint.v1".into(),
                    data: serde_json::json!({"encrypted_content":"synthetic-checkpoint"}),
                }],
            )
        } else {
            CanonicalMessage::text(MessageRole::Assistant, "synthetic helper")
        };
        Ok(Box::pin(futures_util::stream::iter([Ok(
            ModelStreamEvent::Response {
                response: CanonicalModelResponse::new(message, vec![], FinishReason::Stop)
                    .with_usage(TokenUsage::new(3, 2)),
            },
        )])))
    }
}

struct NestedPreparation;
#[async_trait]
impl HookHandler for NestedPreparation {
    async fn invoke(&self, input: HookInput, host: Arc<dyn HookHost>) -> Result<HookResponse> {
        if let HookEvent::BeforeContext {
            input,
            model_capabilities,
            ..
        } = input.event
        {
            assert!(model_capabilities.unwrap().supports_compaction);
            let template = input.request.unwrap();
            let parts = CanonicalMessage::new(
                MessageRole::Assistant,
                vec![ContentPart::Opaque {
                    format: "fixture/checkpoint".into(),
                    data: serde_json::json!({"encrypted":"unchanged"}),
                }],
            )
            .parts;
            let estimate = host
                .estimate_model_input(crate::contracts::ModelInputEstimateRequest {
                    model: template.model.clone(),
                    parts,
                })
                .await?
                .unwrap();
            assert_eq!(estimate.parts[0].tokens, 13);
            for operation in [ModelOperation::Compact, ModelOperation::Complete] {
                host.complete_model(template.clone().with_operation(operation))
                    .await?;
            }
        }
        Ok(HookResponse::Continue)
    }
}

#[derive(Default)]
struct Instructions(Mutex<Vec<(ModelCallOrigin, ModelOperation)>>);
#[async_trait]
impl HookHandler for Instructions {
    async fn invoke(&self, input: HookInput, _: Arc<dyn HookHost>) -> Result<HookResponse> {
        let HookEvent::BeforeModel {
            origin,
            mut request,
        } = input.event
        else {
            return Ok(HookResponse::Continue);
        };
        self.0.lock().unwrap().push((origin, request.operation));
        request.instructions.push(InstructionBlock::new(
            InstructionKind::System,
            "configured base instructions",
            255,
        ));
        Ok(HookResponse::ModelContext {
            instructions: request.instructions,
            messages: request.messages,
        })
    }
}

#[derive(Default)]
struct Recorder {
    requests: Mutex<Vec<(ModelCallOrigin, OperationOwner, CanonicalModelRequest)>>,
    traces: Mutex<Vec<HookTrace>>,
    usage: Mutex<Vec<TokenUsage>>,
}
#[async_trait]
impl ExecutionRecorder for Recorder {
    async fn model_request_recorded(
        &self,
        _: ExchangeId,
        origin: ModelCallOrigin,
        owner: OperationOwner,
        request: &CanonicalModelRequest,
    ) -> Result<()> {
        self.requests
            .lock()
            .unwrap()
            .push((origin, owner, request.clone()));
        Ok(())
    }
    async fn model_response_recorded(
        &self,
        _: ExchangeId,
        response: &CanonicalModelResponse,
    ) -> Result<()> {
        self.usage
            .lock()
            .unwrap()
            .push(response.usage.clone().unwrap());
        Ok(())
    }
    async fn model_error_recorded(
        &self,
        _: ExchangeId,
        failure: &crate::model_standard::ModelFailure,
    ) -> Result<()> {
        panic!("unexpected model failure: {}", failure.message)
    }
    async fn hook_recorded(&self, trace: &HookTrace) -> Result<()> {
        self.traces.lock().unwrap().push(trace.clone());
        Ok(())
    }
}

#[tokio::test]
async fn registry_compaction_callbacks_use_configured_hooks_without_a_strong_reference_cycle() {
    let provider = Arc::new(Provider::default());
    let instructions = Arc::new(Instructions::default());
    let config = crate::core::ModelConfig {
        provider: "fixture".into(),
        model: "native".into(),
        ..Default::default()
    };
    let model_ref = config.model_ref();
    let registry = RuntimeRegistry {
        config_exports: vec![],
        hook_subscriptions: Default::default(),
        hooks: vec![
            ("prepare".into(), Arc::new(NestedPreparation)),
            ("instructions".into(), instructions.clone()),
        ],
        cwd: "/workspace".into(),
        model_config: Some(config),
        runtime_config: Default::default(),
        instructions: vec![],
        model_service: Some(Arc::new(ModelService::new(provider.clone()))),
        context: Arc::new(EmptyContextBuilder),
        tools: ToolRegistry::new(),
        tool_allowlist: None,
        tool_inventory: Arc::new(vec![]),
        process_tool_specs: vec![],
        policy: Arc::new(DenyAllPolicy),
        agent_control: None,
        workflow: Arc::new(NoWorkflow),
        session_storage: None,
        context_providers: vec![],
        mcp_servers: vec![],
        plugins: vec![],
    };
    let scope = ExecutionScope::fresh(Default::default());
    let attribution = ExecutionAttribution::detached(scope.execution_id);
    let recorder = Arc::new(Recorder::default());
    let execution = registry
        .execution_context(
            ModelExecutionBinding::with_recorder(scope, recorder.clone()),
            Arc::new(HeadlessApprovalTransport),
            PermissionMode::Normal,
        )
        .unwrap();
    let weak_hooks = Arc::downgrade(&execution.hooks);
    let weak_operations = Arc::downgrade(execution.operations.as_ref().unwrap());
    let preparation =
        RuntimeContextPreparation::new(execution.clone(), attribution, registry.cwd.clone(), None);
    let request = CanonicalModelRequest::new(model_ref, vec![]);
    assert!(request.instructions.is_empty());
    preparation
        .prepare(ContextPreparationInput::new(vec![], Some(request.clone())))
        .await
        .unwrap();
    execution
        .require_model()
        .unwrap()
        .complete(request)
        .await
        .unwrap();
    let requests = provider.0.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].operation, ModelOperation::Compact);
    assert!(
        !requests[0].instructions.is_empty(),
        "nested Compact lost its configured BeforeModel transform"
    );
    assert!(
        requests[1].instructions.is_empty(),
        "ordinary auxiliary Complete re-entered BeforeModel hooks"
    );
    assert_eq!(requests[0].instructions, requests[2].instructions);
    assert_eq!(
        *instructions.0.lock().unwrap(),
        [
            (ModelCallOrigin::Auxiliary, ModelOperation::Compact),
            (ModelCallOrigin::Direct, ModelOperation::Complete),
        ]
    );
    let recorded = recorder.requests.lock().unwrap();
    assert!(matches!(recorded[0].1, OperationOwner::Hook { .. }));
    assert!(matches!(recorded[1].1, OperationOwner::Hook { .. }));
    assert_eq!(recorded[2].1, OperationOwner::Workflow);
    assert_eq!(recorder.usage.lock().unwrap().len(), 3);
    assert!(recorder.traces.lock().unwrap().iter().any(|trace| {
        matches!(&trace.input.event,HookEvent::BeforeModel { origin:ModelCallOrigin::Auxiliary,request }
            if request.operation == ModelOperation::Compact)
    }));
    drop(preparation);
    drop(execution);
    assert!(
        weak_hooks.upgrade().is_none(),
        "execution hook chain leaked"
    );
    assert!(
        weak_operations.upgrade().is_none(),
        "operation binding leaked"
    );
}
