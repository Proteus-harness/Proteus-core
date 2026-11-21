use super::*;
use crate::{
    contracts::{ExecutionHooks, HookEvent, HookInput},
    model_standard::{
        CanonicalPart, InstructionBlock, InstructionKind, ModelOperation, PartProvenance, PartScope,
    },
};

#[derive(Default)]
struct NativeAdapter {
    supported: bool,
    requests: std::sync::Mutex<Vec<CanonicalModelRequest>>,
}
#[async_trait]
impl Model for NativeAdapter {
    fn id(&self) -> std::borrow::Cow<'static, str> {
        "native-probe".into()
    }
    fn capabilities(&self, _: &ModelRef) -> Result<ModelCapabilities> {
        Ok(ModelCapabilities::empty().with_compaction(self.supported))
    }
    async fn stream(&self, request: CanonicalModelRequest) -> Result<ModelEventStream> {
        let operation = request.operation;
        self.requests.lock().unwrap().push(request);
        let response = if operation == ModelOperation::Compact {
            CanonicalModelResponse::new(
                CanonicalMessage::from_parts(
                    MessageRole::Assistant,
                    vec![CanonicalPart::new(
                        PartProvenance::Model,
                        PartScope::Conversation,
                        ContentPart::Opaque {
                            format: "org.example/native-checkpoint.v1".into(),
                            data: serde_json::json!({"encrypted_content":"synthetic-checkpoint"}),
                        },
                    )],
                ),
                vec![],
                FinishReason::Stop,
            )
        } else {
            response("ok")
        };
        let mut events = Vec::new();
        if operation == ModelOperation::Compact {
            events.push(Ok(ModelStreamEvent::TextDelta {
                message_id: response.messages[0].id,
                phase: None,
                text: "invisible compact delta".into(),
            }));
            events.push(Ok(ModelStreamEvent::ReasoningSummaryDelta {
                text: "invisible compact reasoning".into(),
            }));
        }
        events.push(Ok(ModelStreamEvent::Response { response }));
        Ok(Box::pin(stream::iter(events)))
    }
}

#[derive(Default)]
struct InstructionHook(std::sync::Mutex<Vec<(ModelCallOrigin, ModelOperation)>>);
#[async_trait]
impl ExecutionHooks for InstructionHook {
    async fn apply(&self, input: HookInput) -> Result<HookEvent> {
        let HookEvent::BeforeModel {
            origin,
            mut request,
        } = input.event
        else {
            panic!("BeforeModel expected")
        };
        self.0.lock().unwrap().push((origin, request.operation));
        request.instructions.push(InstructionBlock::new(
            InstructionKind::System,
            "configured base instructions",
            255,
        ));
        Ok(HookEvent::BeforeModel { origin, request })
    }
}

#[tokio::test]
async fn native_auxiliary_compaction_runs_configured_hooks_without_reentering_plain_auxiliary_calls()
 {
    let adapter = Arc::new(NativeAdapter {
        supported: true,
        ..Default::default()
    });
    let hooks = Arc::new(InstructionHook::default());
    let recorder = Arc::new(CollectingExecutionRecorder::default());
    let scope = ExecutionScope::fresh(CancellationToken::new());
    let model = BoundModel::new(
        Arc::new(ModelService::new(adapter.clone())),
        ModelExecutionBinding::with_recorder(scope.clone(), recorder.clone()),
        0,
    )
    .with_hooks(
        hooks.clone(),
        ExecutionAttribution::detached(scope.execution_id),
        ".".into(),
    );
    let cases = [
        (ModelCallOrigin::Direct, ModelOperation::Complete),
        (ModelCallOrigin::Auxiliary, ModelOperation::Complete),
        (ModelCallOrigin::Auxiliary, ModelOperation::Compact),
        (ModelCallOrigin::Direct, ModelOperation::Compact),
    ];
    for (origin, operation) in cases {
        crate::core::model_call_scope::with_model_call_origin(
            origin,
            model.complete(request("native-probe", "case").with_operation(operation)),
        )
        .await
        .unwrap();
    }
    assert_eq!(*hooks.0.lock().unwrap(), [cases[0], cases[2], cases[3]]);
    let requests = adapter.requests.lock().unwrap();
    for (index, request) in requests.iter().enumerate() {
        assert_eq!(request.instructions.len(), usize::from(index != 1));
        assert_eq!(request.operation, cases[index].1);
    }
    let facts = recorder.facts.lock().await;
    assert_eq!(facts.requests.len(), 4);
    assert_eq!(facts.responses.len(), 4);
    for (record, (origin, operation)) in facts.requests.iter().zip(cases) {
        assert_eq!(record.1, origin);
        assert_eq!(record.2.operation, operation);
    }
}

#[tokio::test]
async fn unsupported_native_compaction_is_rejected_before_hooks_and_provider_io() {
    let adapter = Arc::new(NativeAdapter::default());
    let hooks = Arc::new(InstructionHook::default());
    let scope = ExecutionScope::fresh(CancellationToken::new());
    let model = BoundModel::new(
        Arc::new(ModelService::new(adapter.clone())),
        ModelExecutionBinding::detached(scope.clone()),
        0,
    )
    .with_hooks(
        hooks.clone(),
        ExecutionAttribution::detached(scope.execution_id),
        ".".into(),
    );
    let error = model
        .complete(request("native-probe", "unsupported").with_operation(ModelOperation::Compact))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("does not support native compaction")
    );
    assert!(hooks.0.lock().unwrap().is_empty());
    assert!(adapter.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_compaction_is_not_presented_even_with_scalar_metadata() {
    for metadata in [
        serde_json::json!("scalar"),
        serde_json::json!(17),
        serde_json::json!([false]),
    ] {
        for operation in [ModelOperation::Complete, ModelOperation::Compact] {
            let adapter = Arc::new(NativeAdapter {
                supported: true,
                ..Default::default()
            });
            let sink = Arc::new(CollectingSink::default());
            let recorder = Arc::new(CollectingExecutionRecorder::default());
            let scope = ExecutionScope::fresh(CancellationToken::new());
            let model = BoundModel::new(
                Arc::new(ModelService::new(adapter)),
                ModelExecutionBinding::for_turn(
                    scope,
                    Arc::new(EventEmitter::new(sink.clone())),
                    new_session_id(),
                    new_thread_id(),
                    new_turn_id(),
                    recorder.clone(),
                ),
                0,
            );
            model
                .complete(
                    request("native-probe", "presentation")
                        .with_operation(operation)
                        .with_metadata(metadata.clone()),
                )
                .await
                .unwrap();
            assert_eq!(
                sink.events.lock().await.is_empty(),
                operation == ModelOperation::Compact
            );
            let facts = recorder.facts.lock().await;
            assert_eq!(facts.requests[0].2.metadata, metadata);
            assert_eq!(facts.responses.len(), 1);
        }
    }
}
