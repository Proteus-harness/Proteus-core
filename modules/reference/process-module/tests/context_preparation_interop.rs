//! Swappable compactors obey canonical scopes and typed report fields,
//! independently of descriptive message names and module-specific diagnostics.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use proteus_contracts::{
    contracts::{
        ContextPreparationInput, ContextPreparationOutput, ExecutionAttribution, HookEvent,
        HookHost, HookInput, HookResponse,
    },
    domain::{
        ContextChunk, ContextRenderMode, HistoryCompactionReport, ModelRef, ToolCall, ToolResult,
        new_execution_id,
    },
    model_standard::{
        CanonicalMessage, CanonicalModelRequest, CanonicalModelResponse, CanonicalPart,
        ContentPart, FinishReason, MessageRole, PartProvenance, PartScope,
    },
};
use proteus_core::core::{AppConfig, RuntimeRegistry};
use proteus_module_protocol::{
    ProcessComponentBinding, ProcessExportBinding, current_process_contract_authority,
    v3::{ComponentBroker, ComponentBrokerOptions, InvocationTerminal},
};
use proteus_process_host::ProcessSpec;
use serde_json::{Value, json};

fn strategy(module_id: &str) -> Value {
    if module_id == "codex_compact" {
        json!({"trigger_tokens": 100})
    } else {
        json!({"trigger_messages": 3, "retain_user_turns": 1})
    }
}

struct SummaryHost;

struct DelayedSummaryHost;
struct NoModelHost;
#[async_trait]
impl HookHost for NoModelHost {
    async fn complete_model(
        &self,
        _request: CanonicalModelRequest,
    ) -> anyhow::Result<CanonicalModelResponse> {
        anyhow::bail!("standalone preparation has no model")
    }
    async fn execute_tool(&self, _call: ToolCall) -> anyhow::Result<ToolResult> {
        anyhow::bail!("standalone preparation has no tools")
    }
}

#[async_trait]
impl HookHost for DelayedSummaryHost {
    async fn execute_tool(&self, _call: ToolCall) -> anyhow::Result<ToolResult> {
        anyhow::bail!("summary fixture does not execute tools")
    }

    async fn complete_model(
        &self,
        request: CanonicalModelRequest,
    ) -> anyhow::Result<CanonicalModelResponse> {
        tokio::time::sleep(Duration::from_millis(500)).await;
        SummaryHost.complete_model(request).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_compactor_budget_limits_the_whole_model_callback() {
    let workspace = tempfile::tempdir().unwrap();
    for (budget, succeeds) in [(200, false), (5_000, true)] {
        let mut value = serde_json::to_value(config("codex_compact")).unwrap();
        value["components"]["swappable-compactor"]["exports"]["hook"]["codex_compact"]["timeout_ms"] =
            json!(budget);
        let config: AppConfig = serde_json::from_value(value).unwrap();
        let cwd = workspace.path().to_path_buf();
        let registry = tokio::task::spawn_blocking({
            let cwd = cwd.clone();
            move || RuntimeRegistry::from_config(&config, cwd)
        })
        .await
        .unwrap()
        .unwrap();
        let input = ContextPreparationInput::new(
            history(),
            Some(CanonicalModelRequest::new(
                ModelRef::new("fake", "fixture"),
                history(),
            )),
        )
        .with_token_estimate(Some(100_000));
        let result = prepare(
            &registry,
            input,
            workspace.path(),
            Arc::new(DelayedSummaryHost),
        )
        .await;
        if succeeds {
            assert!(result.unwrap().changed);
        } else {
            assert!(format!("{:#}", result.unwrap_err()).contains("timed out"));
        }
    }
}

#[async_trait]
impl HookHost for SummaryHost {
    async fn execute_tool(&self, _call: ToolCall) -> anyhow::Result<ToolResult> {
        anyhow::bail!("summary fixture does not execute tools")
    }

    async fn complete_model(
        &self,
        _request: CanonicalModelRequest,
    ) -> anyhow::Result<CanonicalModelResponse> {
        Ok(CanonicalModelResponse::new(
            CanonicalMessage::text(MessageRole::Assistant, "Continue the current task."),
            vec![],
            FinishReason::Stop,
        ))
    }
}

fn hook_input(input: ContextPreparationInput, cwd: &Path) -> HookInput {
    HookInput {
        event: HookEvent::BeforeContext {
            output: ContextPreparationOutput::unchanged(input.messages.clone()),
            input,
            target: None,
        },
        attribution: ExecutionAttribution::detached(new_execution_id()),
        cwd: cwd.to_path_buf(),
        conversation: None,
    }
}
async fn prepare(
    registry: &RuntimeRegistry,
    input: ContextPreparationInput,
    cwd: &Path,
    host: Arc<dyn HookHost>,
) -> anyhow::Result<ContextPreparationOutput> {
    let original = input.messages.clone();
    match registry.hooks[0]
        .1
        .invoke(hook_input(input, cwd), host)
        .await?
    {
        HookResponse::ContextPreparation { output } => Ok(output),
        HookResponse::Continue => Ok(ContextPreparationOutput::unchanged(original)),
        _ => anyhow::bail!("unexpected preparation response"),
    }
}

fn python_worker() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/modules/context-preparation-process/prepare.py")
}

fn config(module_id: &str) -> AppConfig {
    let command = if module_id == "codex_compact" {
        json!({"command": env!("CARGO_BIN_EXE_proteus-reference-module")})
    } else {
        json!({"command": "python3", "args": [python_worker()],
            "env": {"PYTHONDONTWRITEBYTECODE": "1"}})
    };
    let mut component = command;
    component["exports"] = json!({"hook": {module_id: {"timeout_ms": 5000}}});
    let mut config = AppConfig::default();
    config
        .module_config
        .entry("model".into())
        .or_default()
        .insert("fake".into(), json!({"implementation": "fake"}));
    config.components.insert(
        "model".into(),
        serde_json::from_value(json!({
            "command": env!("CARGO_BIN_EXE_proteus-reference-module"),
            "exports": {"model": {"fake": {}}}
        }))
        .unwrap(),
    );
    config.modules.hooks = vec![module_id.to_owned()];
    config
        .module_config
        .entry("hook".to_owned())
        .or_default()
        .insert(module_id.to_owned(), strategy(module_id));
    config.components.insert(
        "swappable-compactor".to_owned(),
        serde_json::from_value(component).unwrap(),
    );
    config
}

fn history() -> Vec<CanonicalMessage> {
    vec![
        // Mixed payloads: retention comes from scope, not a context-pack marker;
        // both compactors must also preserve the explicit model render mode.
        CanonicalMessage::from_parts(
            MessageRole::User,
            vec![
                CanonicalPart::new(
                    PartProvenance::ContextBuilder,
                    PartScope::Request,
                    ContentPart::Text {
                        text: "Fresh workspace instructions.".to_owned(),
                    },
                ),
                CanonicalPart::new(
                    PartProvenance::ContextBuilder,
                    PartScope::Request,
                    ContentPart::Context {
                        chunk: ContextChunk::new("external", "Exact instructions.\n")
                            .with_render_mode(ContextRenderMode::Verbatim),
                    },
                ),
            ],
        )
        .with_name("context"),
        CanonicalMessage::text(MessageRole::User, "Old task."),
        CanonicalMessage::text(MessageRole::Assistant, "Old answer."),
        CanonicalMessage::text(MessageRole::User, "Current task."),
    ]
}

async fn check_names(module_id: &str) {
    let workspace = tempfile::tempdir().unwrap();
    let cwd = workspace.path().to_path_buf();
    let registry = tokio::task::spawn_blocking({
        let cwd = cwd.clone();
        let config = config(module_id);
        move || RuntimeRegistry::from_config(&config, cwd)
    })
    .await
    .unwrap()
    .unwrap();
    let original = history();
    let mut baseline = None;
    for (case, rename) in [
        ("baseline", None),
        (
            "renamed request context",
            Some((0, Some("another-workflow-context"))),
        ),
        ("unnamed request context", Some((0, None))),
        ("current user named context", Some((3, Some("context")))),
        ("old user named context", Some((1, Some("context")))),
    ] {
        let mut messages = original.clone();
        if let Some((index, name)) = rename {
            messages[index].name = name.map(str::to_owned);
        }
        let input = ContextPreparationInput::new(
            messages.clone(),
            (module_id == "codex_compact").then(|| {
                CanonicalModelRequest::new(ModelRef::new("fake", "fixture"), messages.clone())
            }),
        )
        .with_token_estimate(Some(100_000));
        let host: Arc<dyn HookHost> = if module_id == "codex_compact" {
            Arc::new(SummaryHost)
        } else {
            Arc::new(NoModelHost)
        };
        let output = prepare(&registry, input.clone(), &cwd, host).await.unwrap();
        assert!(output.changed, "{module_id}: {case} must still compact");
        let report = HistoryCompactionReport::from_context_preparation_output(&input, &output);
        assert_eq!(report.input_messages, messages.len());
        assert_eq!(report.output_messages, output.messages.len());
        assert_eq!(report.original_token_estimate, Some(100_000));
        assert_eq!(report.output_token_estimate, output.token_estimate);
        assert_eq!(report.skipped_reason, None);
        if module_id == "codex_compact" {
            assert_eq!(report.trigger_tokens, Some(100));
            assert_eq!(report.summary_source.as_deref(), Some("model"));
            assert!(report.output_token_estimate.is_some());
        } else {
            // A message-count strategy has no token trigger to invent.
            assert_eq!(report.trigger_tokens, None);
            assert_eq!(
                report.summary_source.as_deref(),
                Some("deterministic_suffix")
            );
        }
        for key in [
            "input_messages",
            "output_messages",
            "original_token_estimate",
            "output_token_estimate",
            "trigger_tokens",
            "summary_source",
            "user_message_replacements",
            "skipped_reason",
        ] {
            assert!(
                output.metadata.get(key).is_none(),
                "{module_id}: duplicated {key}"
            );
        }
        for index in [0, 3] {
            assert!(
                output.messages.contains(&messages[index]),
                "{module_id}: {case} lost or changed message {index}"
            );
        }
        // Strategies may retain different history and create their own summary.
        // Only the same strategy under label-only changes must be equivalent.
        let retained: Vec<_> = output
            .messages
            .iter()
            .filter(|message| original.iter().any(|input| input.id == message.id))
            .map(|message| {
                let mut canonical = message.clone();
                canonical.name = None;
                canonical
            })
            .collect();
        if let Some(expected) = &baseline {
            assert_eq!(&retained, expected, "{module_id}: {case} changed retention");
        } else {
            baseline = Some(retained);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_compactor_uses_scope_not_workflow_message_names() {
    check_names("codex_compact").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn python_preparation_is_model_free_and_uses_scope_not_workflow_message_names() {
    check_names("python_suffix").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn python_compactor_rejects_invalid_launch_settings_and_canonical_scope() {
    let workspace = tempfile::tempdir().unwrap();
    let version = current_process_contract_authority("hook")
        .unwrap()
        .contract_version;
    let invalid_export = ProcessExportBinding::new(
        "hook",
        "python_suffix",
        version,
        json!({"retain_user_turns": 0}),
    )
    .unwrap();
    let invalid_binding =
        ProcessComponentBinding::new("invalid-python-compactor", [invalid_export]).unwrap();
    let invalid_spec = ProcessSpec::new("python3")
        .arg(python_worker().display().to_string())
        .cwd(workspace.path())
        .env("PYTHONDONTWRITEBYTECODE", "1");
    let error = tokio::task::spawn_blocking(move || {
        ComponentBroker::connect(
            invalid_spec,
            invalid_binding,
            ComponentBrokerOptions::default(),
        )
    })
    .await
    .unwrap()
    .err()
    .expect("invalid strategy must fail initialize before any compaction invocation");
    assert!(
        format!("{error:#}").contains("retain_user_turns must be a positive integer"),
        "{error:#}"
    );
    let export = ProcessExportBinding::new("hook", "python_suffix", version, json!({})).unwrap();
    let target = export.export_ref();
    let binding = ProcessComponentBinding::new("python-compactor", [export]).unwrap();
    let spec = ProcessSpec::new("python3")
        .arg(python_worker().display().to_string())
        .cwd(workspace.path())
        .env("PYTHONDONTWRITEBYTECODE", "1");
    let broker = tokio::task::spawn_blocking(move || {
        ComponentBroker::connect(spec, binding, ComponentBrokerOptions::default())
    })
    .await
    .unwrap()
    .unwrap();
    let input = ContextPreparationInput::new(history(), None);
    let valid = serde_json::to_value(hook_input(input, workspace.path())).unwrap();
    for scope in [
        None,
        Some(json!("unknown")),
        Some(Value::Null),
        Some(json!([])),
    ] {
        let mut invalid = valid.clone();
        let part = invalid["event"]["input"]["messages"][0]["parts"][0]
            .as_object_mut()
            .unwrap();
        if let Some(scope) = scope {
            part.insert("scope".to_owned(), scope);
        } else {
            part.remove("scope");
        }
        let terminal = broker
            .invoke(&target, "hook.invoke", invalid, Duration::from_secs(5))
            .await
            .unwrap();
        assert!(
            matches!(terminal, InvocationTerminal::ModuleError(ref error)
            if error.message.contains("canonical part scope")),
            "{terminal:?}"
        );
    }
    // Malformed input must neither crash the component nor cause a permissive
    // fallback. The same worker still accepts a correctly marked request.
    let terminal = broker
        .invoke(&target, "hook.invoke", valid, Duration::from_secs(5))
        .await
        .unwrap();
    assert!(
        matches!(terminal, InvocationTerminal::Success(_)),
        "{terminal:?}"
    );
}
