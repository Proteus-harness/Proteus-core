use super::*;
use crate::{
    contracts::{
        ApprovalPolicy, CancellationToken, ExecutionHooks, ExecutionScope, Model, ModelEventStream,
        PolicyContext, PolicyVisibilityContext, ToolRegistry,
    },
    core::HeadlessApprovalTransport,
    domain::{ModelRef, PolicyDecision, ToolCall},
    model_standard::{CanonicalModelRequest, ModelCapabilities},
};
use std::sync::Mutex;

struct UnusedPolicy;
impl ApprovalPolicy for UnusedPolicy {
    fn evaluate(&self, _: &ToolCall, _: &PolicyContext) -> PolicyDecision {
        panic!("preparation must not execute tools")
    }
    fn evaluate_visibility(&self, _: &PolicyVisibilityContext) -> PolicyDecision {
        panic!("preparation must not select tools")
    }
}

#[derive(Default)]
struct DescribedModel(Mutex<Vec<ModelRef>>);
#[async_trait]
impl Model for DescribedModel {
    fn id(&self) -> std::borrow::Cow<'static, str> {
        "description-probe".into()
    }
    fn capabilities(&self, model: &ModelRef) -> Result<ModelCapabilities> {
        self.0.lock().unwrap().push(model.clone());
        Ok(ModelCapabilities::empty().with_compaction(model.model == "native"))
    }
    async fn stream(&self, _: CanonicalModelRequest) -> Result<ModelEventStream> {
        panic!("preparation description must not generate")
    }
}

#[derive(Default)]
struct CaptureHook {
    inputs: Mutex<Vec<HookInput>>,
    tamper: bool,
}
#[async_trait]
impl ExecutionHooks for CaptureHook {
    async fn apply(&self, input: HookInput) -> Result<HookEvent> {
        self.inputs.lock().unwrap().push(input.clone());
        let mut event = input.event;
        if self.tamper {
            let HookEvent::BeforeContext {
                model_capabilities, ..
            } = &mut event
            else {
                panic!("preparation event")
            };
            *model_capabilities = None;
        }
        Ok(event)
    }
}

fn preparation(model: Arc<DescribedModel>, hooks: Arc<CaptureHook>) -> RuntimeContextPreparation {
    let scope = ExecutionScope::fresh(CancellationToken::new());
    let attribution = ExecutionAttribution::detached(scope.execution_id);
    let execution = ExecutionContext::new(
        scope,
        0,
        Some(model),
        ToolRegistry::new(),
        Arc::new(UnusedPolicy),
        Arc::new(HeadlessApprovalTransport),
    )
    .with_hooks(hooks);
    RuntimeContextPreparation::new(execution, attribution, ".".into(), None)
}

#[tokio::test]
async fn capability_is_described_by_the_host_for_the_selected_model() {
    let model = Arc::new(DescribedModel::default());
    let hooks = Arc::new(CaptureHook::default());
    let preparation = preparation(model.clone(), hooks.clone());
    for name in ["native", "local"] {
        let request = CanonicalModelRequest::new(ModelRef::new("provider", name), vec![]);
        preparation
            .prepare(ContextPreparationInput::new(vec![], Some(request)))
            .await
            .unwrap();
    }
    let inputs = hooks.inputs.lock().unwrap();
    for (input, expected) in inputs.iter().zip([true, false]) {
        let HookEvent::BeforeContext {
            model_capabilities: Some(capabilities),
            ..
        } = &input.event
        else {
            panic!("bound capabilities missing")
        };
        assert_eq!(capabilities.supports_compaction, expected);
    }
    assert_eq!(
        model
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|m| m.model.as_str())
            .collect::<Vec<_>>(),
        ["native", "local"]
    );
}

#[tokio::test]
async fn model_free_preparation_does_not_describe_the_configured_model() {
    let model = Arc::new(DescribedModel::default());
    let hooks = Arc::new(CaptureHook::default());
    preparation(model.clone(), hooks.clone())
        .prepare(ContextPreparationInput::new(vec![], None))
        .await
        .unwrap();
    assert!(model.0.lock().unwrap().is_empty());
    assert!(matches!(
        &hooks.inputs.lock().unwrap()[0].event,
        HookEvent::BeforeContext {
            model_capabilities: None,
            ..
        }
    ));
}

#[tokio::test]
async fn a_request_template_does_not_require_a_model_for_deterministic_preparation() {
    let hooks = Arc::new(CaptureHook::default());
    let mut preparation = preparation(Arc::new(DescribedModel::default()), hooks.clone());
    preparation.execution.model = None;
    let request = CanonicalModelRequest::new(ModelRef::new("unconfigured", "template"), vec![]);
    preparation
        .prepare(ContextPreparationInput::new(vec![], Some(request)))
        .await
        .unwrap();
    assert!(matches!(
        &hooks.inputs.lock().unwrap()[0].event,
        HookEvent::BeforeContext {
            model_capabilities: None,
            ..
        }
    ));
}

#[tokio::test]
async fn preparation_rejects_a_changed_host_capability() {
    let hooks = Arc::new(CaptureHook {
        tamper: true,
        ..Default::default()
    });
    let request = CanonicalModelRequest::new(ModelRef::new("provider", "native"), vec![]);
    let error = preparation(Arc::new(DescribedModel::default()), hooks)
        .prepare(ContextPreparationInput::new(vec![], Some(request)))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("changed bound input or target"));
}

#[tokio::test]
async fn cancelled_preparation_has_no_description_or_hook_side_effect() {
    let model = Arc::new(DescribedModel::default());
    let hooks = Arc::new(CaptureHook::default());
    let preparation = preparation(model.clone(), hooks.clone());
    preparation.execution.scope.cancellation.cancel();
    let request = CanonicalModelRequest::new(ModelRef::new("provider", "native"), vec![]);
    assert!(
        preparation
            .prepare(ContextPreparationInput::new(vec![], Some(request)))
            .await
            .is_err()
    );
    assert!(model.0.lock().unwrap().is_empty());
    assert!(hooks.inputs.lock().unwrap().is_empty());
}
