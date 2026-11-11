//! Same-ID host callbacks must be rejected before a nonidempotent effect.
use std::{io::Write, path::PathBuf};

use super::*;
use crate::{
    contracts::{
        ApprovalPolicy, PolicyContext, PolicyVisibilityContext, Tool, ToolContext, ToolRegistry,
        ToolSource, WorkflowHistoryCheckpoint, WorkflowHistoryUpdate, WorkflowToolResultBinding,
    },
    core::workflow_host::WorkflowHostRuntime,
    domain::{PolicyDecision, ToolCall, ToolResult, ToolSafety, ToolSpec},
    model_standard::ContentPart,
};

struct AppendEffectTool(PathBuf);

#[async_trait]
impl Tool for AppendEffectTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "append_effect",
            "Append one irreversible test effect",
            serde_json::json!({"type": "object"}),
            ToolSafety::WritesFiles,
        )
    }

    async fn invoke(&self, call: &ToolCall, _ctx: ToolContext) -> Result<ToolResult> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.0)?;
        writeln!(file, "effect")?;
        // Allow the other host callback to reach admission before this result
        // is recorded. A result-time duplicate check cannot protect the file.
        tokio::task::yield_now().await;
        Ok(ToolResult::ok(call.id.clone(), "appended"))
    }
}

struct AllowEffects;

impl ApprovalPolicy for AllowEffects {
    fn evaluate(&self, _call: &ToolCall, _ctx: &PolicyContext) -> PolicyDecision {
        PolicyDecision::Allow
    }

    fn evaluate_visibility(&self, _ctx: &PolicyVisibilityContext) -> PolicyDecision {
        PolicyDecision::Allow
    }
}

struct RepeatedCallbackWorkflow {
    concurrent: bool,
}

fn call_message(call: &ToolCall) -> CanonicalMessage {
    CanonicalMessage::new(
        MessageRole::Assistant,
        vec![ContentPart::ToolCall { call: call.clone() }],
    )
}

fn assert_duplicate(error: anyhow::Error) {
    assert!(
        format!("{error:#}").contains("tool call original was requested more than once"),
        "must reject at admission, before invoking the tool: {error:#}"
    );
}

#[async_trait]
impl Workflow for RepeatedCallbackWorkflow {
    async fn run(
        &self,
        task: AgentTask,
        history: Vec<CanonicalMessage>,
        ctx: crate::contracts::WorkflowInvocationContext,
    ) -> Result<WorkflowOutput> {
        let host = WorkflowHostRuntime::new(ctx, task.cwd.clone());
        let original = ToolCall::new("original", "append_effect", serde_json::json!({}));
        host.checkpoint_history(WorkflowHistoryCheckpoint {
            history: WorkflowHistoryUpdate {
                new_messages: vec![call_message(&original)],
                history_replacement: None,
                compactions: Vec::new(),
            },
            tool_results: vec![WorkflowToolResultBinding::new(original.clone())],
            preparation_target: None,
        })
        .await?;

        if self.concurrent {
            let (first, second) = tokio::join!(
                host.execute_tool(task.clone(), original.clone()),
                host.execute_tool(task.clone(), original.clone()),
            );
            match (first, second) {
                (Ok(result), Err(error)) | (Err(error), Ok(result)) => {
                    assert!(result.ok);
                    assert_duplicate(error);
                }
                other => panic!("one same-ID callback must be rejected: {other:?}"),
            }
        } else {
            assert!(host.execute_tool(task.clone(), original.clone()).await?.ok);
            assert_duplicate(
                host.execute_tool(task.clone(), original.clone())
                    .await
                    .expect_err("repeated callback"),
            );
        }

        // Drop the call and result from model history. Admission identity must
        // survive the checkpoint's new capture and compacted transcript.
        let mut report = HistoryCompactionReport::unchanged(history.len() + 2, None);
        report.changed = true;
        report.output_messages = history.len();
        host.checkpoint_history(WorkflowHistoryCheckpoint {
            history: WorkflowHistoryUpdate {
                new_messages: Vec::new(),
                history_replacement: Some(history.clone()),
                compactions: vec![report.clone()],
            },
            tool_results: Vec::new(),
            preparation_target: None,
        })
        .await?;
        assert_duplicate(
            host.execute_tool(task.clone(), original.clone())
                .await
                .expect_err("repeated callback after compaction"),
        );

        // This is intentional repetition of the same command with a fresh
        // canonical identity. Only call identity, never arguments, deduplicates.
        let fresh = ToolCall::new("fresh", &original.name, original.args.clone());
        host.checkpoint_history(WorkflowHistoryCheckpoint {
            history: WorkflowHistoryUpdate {
                new_messages: vec![call_message(&fresh)],
                history_replacement: Some(history),
                compactions: vec![report],
            },
            tool_results: vec![WorkflowToolResultBinding::new(fresh.clone())],
            preparation_target: None,
        })
        .await?;
        assert!(host.execute_tool(task, fresh).await?.ok);
        anyhow::bail!("admission probe finished")
    }
}

async fn check_repeated_callback_effects(concurrent: bool) {
    for persist in [false, true] {
        let cwd = tempfile::tempdir().unwrap();
        let effects = cwd.path().join("effects.txt");
        let config_path = cwd.path().join("config.toml");
        let runtime = AgentRuntime::builder(AppConfig::default(), cwd.path().to_owned())
            .with_config_path(persist.then_some(config_path.as_path()))
            .with_module_catalog(test_catalog())
            .build()
            .unwrap();
        assert_eq!(runtime.session_dir().is_some(), persist);
        {
            let mut state = runtime.services.execution_state.write().await;
            let mut tools = ToolRegistry::new();
            tools
                .register_arc(
                    ToolSource::Dynamic {
                        origin: "admission-test".into(),
                    },
                    Arc::new(AppendEffectTool(effects.clone())),
                )
                .unwrap();
            state.runtime.registry.tools = tools;
            state.runtime.registry.policy = Arc::new(AllowEffects);
            state.runtime.registry.workflow = Arc::new(RepeatedCallbackWorkflow { concurrent });
        }
        let error = runtime.run("exercise callbacks".into()).await.unwrap_err();
        assert!(
            format!("{error:#}").contains("admission probe finished"),
            "{error:#}"
        );
        assert_eq!(
            std::fs::read_to_string(effects).unwrap(),
            "effect\neffect\n",
            "only original and fresh IDs may append; persist={persist}, concurrent={concurrent}"
        );
    }
}

#[tokio::test]
async fn sequential_tool_callback_does_not_repeat_effect_across_checkpoint_rebase() {
    check_repeated_callback_effects(false).await;
}

#[tokio::test]
async fn concurrent_tool_callbacks_admit_one_effect_and_retain_identity_after_compaction() {
    check_repeated_callback_effects(true).await;
}
