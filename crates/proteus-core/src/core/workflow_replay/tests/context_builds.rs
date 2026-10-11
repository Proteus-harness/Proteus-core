use super::*;
use crate::{
    contracts::{ContextBuildInput, ContextBuildOutcome},
    core::{ContextRequestRecorded, ContextResponseRecorded},
    domain::{ContextBundle, ContextChunk, ContextRenderMode},
};

#[derive(Clone, Copy)]
enum Mutation {
    None,
    Attribution,
    Scope,
    Cwd,
}

struct ContextProbe {
    tasks: Vec<String>,
    masked_output: Option<AgentOutput>,
    failure: Option<String>,
    mutation: Mutation,
}

#[async_trait]
impl Workflow for ContextProbe {
    async fn run(
        &self,
        task: AgentTask,
        _history: Vec<CanonicalMessage>,
        ctx: crate::contracts::WorkflowInvocationContext,
    ) -> anyhow::Result<WorkflowOutput> {
        let ctx = ctx.into_agent()?;
        let mut attribution = ExecutionAttribution::for_turn(
            ctx.execution.scope.execution_id,
            ctx.session_id,
            ctx.thread_id,
            ctx.turn_id,
        );
        if matches!(self.mutation, Mutation::Attribution) {
            attribution = ExecutionAttribution::detached(attribution.execution_id);
        }
        let mut scope = ctx.execution.scope.clone();
        if matches!(self.mutation, Mutation::Scope) {
            scope.execution_id = new_execution_id();
        }
        let mut cwd = task.cwd;
        if matches!(self.mutation, Mutation::Cwd) {
            cwd.push("changed-directory");
        }
        let calls = self.tasks.iter().map(|text| {
            ctx.context.build(ContextBuildInput::new(
                AgentTask::new(text, cwd.clone()),
                scope.clone(),
                attribution,
            ))
        });
        // All builds may overlap; replay matches exact inputs rather than
        // requiring scheduling order to be reproduced.
        let outcomes = futures_util::future::join_all(calls).await;
        let first_error = outcomes
            .iter()
            .find_map(|result| result.as_ref().err().map(|error| format!("{error:#}")));
        let mut observed = BTreeMap::new();
        for (task, result) in self.tasks.iter().zip(outcomes) {
            let outcome = match result {
                Ok(bundle) => ContextBuildOutcome::Success { bundle },
                Err(error) => ContextBuildOutcome::Error {
                    message: error.to_string(),
                    causes: error.chain().skip(1).map(ToString::to_string).collect(),
                },
            };
            observed.insert(task.clone(), outcome);
        }
        if let Some(failure) = &self.failure {
            let message = first_error
                .ok_or_else(|| anyhow::anyhow!("recorded context failure was not returned"))?;
            anyhow::ensure!(&message == failure, "context failure changed: {message}");
            anyhow::bail!(message);
        }
        let output = self
            .masked_output
            .clone()
            .unwrap_or_else(|| context_output(observed));
        Ok(WorkflowOutput::new(
            output,
            vec![CanonicalMessage::text(
                MessageRole::Assistant,
                "context read",
            )],
        ))
    }
}

fn context_output(outcomes: BTreeMap<String, ContextBuildOutcome>) -> AgentOutput {
    AgentOutput::new(
        "context read",
        serde_json::to_value(outcomes).expect("outcomes"),
    )
}

fn bundle(text: &str, count: u32) -> ContextBuildOutcome {
    ContextBuildOutcome::Success {
        bundle: ContextBundle::new(vec![
            ContextChunk::new("fixture", text)
                .with_path("fixture.txt".into())
                .with_render_mode(ContextRenderMode::Verbatim)
                .with_score(0.75)
                .with_metadata(json!({"extra": text})),
        ])
        .with_summary(format!("summary {text}"))
        .with_token_estimate(count),
    }
}

async fn context_journal(
    builds: &[(String, ContextBuildOutcome)],
    failure: Option<&str>,
) -> TestJournal {
    context_journal_impl(builds, failure, false).await
}

async fn context_journal_impl(
    builds: &[(String, ContextBuildOutcome)],
    failure: Option<&str>,
    incomplete: bool,
) -> TestJournal {
    let config_dir = tempfile::tempdir().expect("config dir");
    let workspace = tempfile::tempdir().expect("workspace");
    let session = new_session_id();
    let thread = new_thread_id();
    let turn = new_turn_id();
    let execution = new_execution_id();
    let attribution = ExecutionAttribution::for_turn(execution, session, thread, turn);
    let store = SessionStore::new(config_dir.path(), workspace.path(), session).expect("store");
    let task = AgentTask::new("read context", workspace.path().into());
    store
        .append_execution_journal_entry(
            attribution,
            JournalEntry::TurnOpened(TurnOpened {
                intent: None,
                task: task.clone(),
                base_history_revision: 0,
                module_epoch: 1,
                config_snapshot: serde_json::to_value(snapshot(&probe_tool_spec()))
                    .expect("snapshot"),
            }),
        )
        .await
        .expect("open");
    store
        .append_history(
            thread,
            Some(turn),
            &[CanonicalMessage::text(MessageRole::User, task.text)],
        )
        .await
        .expect("history");
    // Record overlapping reads, completed in the opposite order.
    let requests = builds
        .iter()
        .map(|(text, outcome)| {
            (
                uuid::Uuid::new_v4(),
                AgentTask::new(text, workspace.path().into()),
                outcome.clone(),
            )
        })
        .collect::<Vec<_>>();
    for (invocation_id, task, _) in &requests {
        store
            .append_execution_journal_entry(
                attribution,
                JournalEntry::ContextRequestRecorded(ContextRequestRecorded {
                    invocation_id: *invocation_id,
                    task: task.clone(),
                }),
            )
            .await
            .expect("request");
    }
    for (invocation_id, _, outcome) in requests.iter().rev() {
        if incomplete && requests.last().is_some_and(|last| last.0 == *invocation_id) {
            continue;
        }
        store
            .append_execution_journal_entry(
                attribution,
                JournalEntry::ContextResponseRecorded(ContextResponseRecorded {
                    invocation_id: *invocation_id,
                    outcome: outcome.clone(),
                }),
            )
            .await
            .expect("response");
    }
    let output = context_output(builds.iter().cloned().collect());
    if failure.is_none() {
        store
            .append_history(
                thread,
                Some(turn),
                &[CanonicalMessage::text(
                    MessageRole::Assistant,
                    "context read",
                )],
            )
            .await
            .expect("assistant history");
    }
    store
        .append_journal_entry(
            thread,
            Some(turn),
            JournalEntry::TurnSettled(TurnSettled {
                status: if failure.is_some() {
                    TurnSettlementStatus::Error
                } else {
                    TurnSettlementStatus::Success
                },
                output: failure.is_none().then_some(output),
                error: failure.map(str::to_owned),
            }),
        )
        .await
        .expect("settle");
    TestJournal {
        _config_dir: config_dir,
        _workspace: workspace,
        store,
        thread_id: thread,
        turn_id: turn,
    }
}

async fn replay_context(journal: &TestJournal, probe: ContextProbe) -> WorkflowReplayReport {
    let mut catalog = ModuleCatalog::new();
    catalog.register_test_workflow(WORKFLOW_ID, Arc::new(probe));
    catalog.register_test_policy(POLICY_ID, Arc::new(ReplayAllowAll));
    replay_workflow(
        journal.store.session_dir(),
        &AppConfig::default(),
        &catalog,
        WorkflowReplayOptions::default(),
    )
    .await
    .expect("context replay")
}

#[tokio::test]
async fn exact_model_free_context_outcomes_replay_even_when_requests_are_reordered() {
    let builds = vec![
        (
            "first".to_owned(),
            bundle("not present in any model request", 37),
        ),
        ("second".to_owned(), bundle("fresh workspace read", 91)),
        (
            "failed".to_owned(),
            ContextBuildOutcome::Error {
                message: "provider read failed".into(),
                causes: vec!["reading config".into(), "file not found".into()],
            },
        ),
    ];
    let journal = context_journal(&builds, None).await;
    let report = replay_context(
        &journal,
        ContextProbe {
            tasks: vec!["failed".into(), "second".into(), "first".into()],
            masked_output: None,
            failure: None,
            mutation: Mutation::None,
        },
    )
    .await;
    assert!(
        report.comparison.matched,
        "{:?} {:?}",
        report.comparison.issues, report.replay
    );
    assert_eq!(report.model_exchanges.recorded, 0);
    assert!(report.source_journal_unchanged);
}

#[tokio::test]
async fn model_free_context_failure_replays_without_a_later_model_request() {
    let failure = "context unavailable before model";
    let builds = vec![(
        "failure".into(),
        ContextBuildOutcome::Error {
            message: failure.into(),
            causes: vec![],
        },
    )];
    let journal = context_journal(&builds, Some(failure)).await;
    let report = replay_context(
        &journal,
        ContextProbe {
            tasks: vec!["failure".into()],
            masked_output: None,
            failure: Some(failure.into()),
            mutation: Mutation::None,
        },
    )
    .await;
    assert!(
        report.comparison.matched,
        "{:?} {:?}",
        report.comparison.issues, report.replay
    );
    assert_eq!(report.comparison.error_equal, Some(true));
    assert!(report.source_journal_unchanged);
}

#[tokio::test]
async fn caught_changed_extra_missing_and_wrong_attribution_builds_still_diverge() {
    let builds = vec![
        ("first".into(), bundle("first", 37)),
        ("second".into(), bundle("second", 91)),
    ];
    let journal = context_journal(&builds, None).await;
    let output = context_output(builds.iter().cloned().collect());
    for (tasks, mutation, issue) in [
        (
            vec!["first", "changed"],
            Mutation::None,
            "task input differs",
        ),
        (
            vec!["first", "second", "extra"],
            Mutation::None,
            "unexpected context build",
        ),
        (vec!["first"], Mutation::None, "context builds unconsumed"),
        (
            vec!["first", "second"],
            Mutation::Attribution,
            "attribution or scope differs",
        ),
        (
            vec!["first", "second"],
            Mutation::Scope,
            "attribution or scope differs",
        ),
        (vec!["first", "second"], Mutation::Cwd, "task input differs"),
    ] {
        let report = replay_context(
            &journal,
            ContextProbe {
                tasks: tasks.into_iter().map(str::to_owned).collect(),
                masked_output: Some(output.clone()),
                failure: None,
                mutation,
            },
        )
        .await;
        assert_eq!(
            report.comparison.output_equal,
            Some(true),
            "{:?}",
            report.replay
        );
        assert!(!report.comparison.matched);
        assert!(
            report
                .comparison
                .issues
                .iter()
                .any(|message| message.contains(issue)),
            "{:?}",
            report.comparison.issues
        );
        assert!(report.source_journal_unchanged);
    }
}

#[tokio::test]
async fn discarded_context_calls_cannot_match_a_model_bearing_turn_without_recorded_builds() {
    struct ExtraContext;
    #[async_trait]
    impl Workflow for ExtraContext {
        async fn run(
            &self,
            task: AgentTask,
            history: Vec<CanonicalMessage>,
            ctx: crate::contracts::WorkflowInvocationContext,
        ) -> anyhow::Result<WorkflowOutput> {
            let agent = ctx.clone().into_agent()?;
            let attribution = ExecutionAttribution::for_turn(
                agent.execution.scope.execution_id,
                agent.session_id,
                agent.thread_id,
                agent.turn_id,
            );
            for _ in 0..2 {
                let _ = agent
                    .context
                    .build(ContextBuildInput::new(
                        task.clone(),
                        agent.execution.scope.clone(),
                        attribution,
                    ))
                    .await;
            }
            ProbeWorkflow { diverge: false }
                .run(task, history, ctx)
                .await
        }
    }
    let journal = TestJournal::new().await;
    let mut catalog = ModuleCatalog::new();
    catalog.register_test_workflow(WORKFLOW_ID, Arc::new(ExtraContext));
    catalog.register_test_policy(POLICY_ID, Arc::new(ReplayAllowAll));
    let report = replay_workflow(
        journal.store.session_dir(),
        &AppConfig::default(),
        &catalog,
        WorkflowReplayOptions::default(),
    )
    .await
    .expect("report");
    assert_eq!(
        report.comparison.output_equal,
        Some(true),
        "{:?}",
        report.replay
    );
    assert_eq!(report.comparison.history_equal, Some(true));
    assert!(!report.comparison.matched);
    assert_eq!(
        report
            .comparison
            .issues
            .iter()
            .filter(|issue| issue.contains("unexpected context build"))
            .count(),
        2
    );
    assert!(report.source_journal_unchanged);
}

#[tokio::test]
async fn incomplete_context_build_cannot_be_used_for_success_or_error_replay() {
    for failure in [None, Some("workflow failure")] {
        let journal =
            context_journal_impl(&[("pending".into(), bundle("pending", 5))], failure, true).await;
        let error = replay_workflow(
            journal.store.session_dir(),
            &AppConfig::default(),
            &catalog(false),
            WorkflowReplayOptions::default(),
        )
        .await
        .expect_err("pending build has no recorded outcome");
        assert!(
            error
                .to_string()
                .contains("is incomplete and cannot be used for workflow replay"),
            "{error:#}"
        );
    }
}

#[tokio::test]
async fn context_failure_replays_complete_error_chain_display() {
    let outcome = ContextBuildOutcome::Error {
        message: "building context".into(),
        causes: vec!["reading provider".into(), "file not found".into()],
    };
    let failure = "building context: reading provider: file not found";
    let journal = context_journal(&[("failure".into(), outcome)], Some(failure)).await;
    let report = replay_context(
        &journal,
        ContextProbe {
            tasks: vec!["failure".into()],
            masked_output: None,
            failure: Some(failure.into()),
            mutation: Mutation::None,
        },
    )
    .await;
    assert!(
        report.comparison.matched,
        "{:?} {:?}",
        report.comparison.issues, report.replay
    );
    assert_eq!(report.comparison.error_equal, Some(true));
}
