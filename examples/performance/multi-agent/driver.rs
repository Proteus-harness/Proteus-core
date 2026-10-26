//! Parent control plane for real, configured process peers. No retained event bodies.
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use proteus_contracts::{
    contracts::{
        AgentControlRequest, AgentLifecycleStatus, AgentWorkflowContext, CancellationToken,
        EventEmitter, EventSink, ExecutionScope,
    },
    domain::{
        AgentTask, Event, EventEnvelope, ModelRef, PermissionMode, ReasoningConfig, ThreadId,
        new_session_id, new_thread_id, new_turn_id,
    },
};
use proteus_core::core::{
    AgentControlConfig, AgentControlRuntime, AppConfig, HeadlessApprovalTransport,
    HeadlessUserInputTransport, ModelExecutionBinding, RuntimeRegistry,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Input {
    binary: PathBuf,
    config: PathBuf,
    workspace: PathBuf,
    peers: usize,
    rounds: usize,
    idle_cap: usize,
}

struct Events {
    start: Instant,
    state: Mutex<Round>,
    peers: usize,
}

#[derive(Default)]
struct Round {
    number: usize,
    entered: HashSet<ThreadId>,
    finished: usize,
    concurrent: bool,
}

impl Events {
    fn mark(&self, phase: &str, extra: Value) {
        println!(
            "{}",
            json!({"phase":phase,"elapsed_ms":self.start.elapsed().as_secs_f64()*1000.,"data":extra})
        );
    }
}

#[async_trait]
impl EventSink for Events {
    async fn append(&self, envelope: EventEnvelope) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        match envelope.event {
            Event::ToolCallRequested { ref call, .. }
                if call.name == "read_file" && state.entered.insert(envelope.thread_id) =>
            {
                self.mark("peer_model_entered", json!({"round":state.number,"thread":envelope.thread_id,"count":state.entered.len(),"source":"forwarded_fake_model_tool_call"}));
                if state.entered.len() == self.peers {
                    state.concurrent = state.finished == 0;
                    self.mark(
                        "all_models_entered",
                        json!({"round":state.number,"before_first_finished":state.concurrent}),
                    );
                }
            }
            Event::SubagentFinished { .. } => state.finished += 1,
            _ => {}
        }
        Ok(())
    }
}

#[tokio::main]
pub async fn run() -> Result<()> {
    let input: Input = serde_json::from_slice(&std::fs::read(
        std::env::args_os().nth(1).context("driver INPUT.json")?,
    )?)?;
    ensure!((1..=10).contains(&input.peers) && (1..=20).contains(&input.rounds));
    ensure!(
        input.rounds == 1 || input.idle_cap >= input.peers,
        "resumed rounds require resident slots for all peers"
    );
    let events = Arc::new(Events {
        start: Instant::now(),
        state: Mutex::new(Round::default()),
        peers: input.peers,
    });
    events.mark(
        "parent_start",
        json!({"peers":input.peers,"rounds":input.rounds,"idle_cap":input.idle_cap}),
    );
    let config = AppConfig::load(Some(&input.config)).await?;
    let registry = RuntimeRegistry::from_config(&config, input.workspace.clone())?;
    let execution = registry.execution_context(
        ModelExecutionBinding::detached(ExecutionScope::fresh(CancellationToken::new())),
        Arc::new(HeadlessApprovalTransport),
        PermissionMode::Normal,
    )?;
    let context = AgentWorkflowContext::new(
        execution,
        new_session_id(),
        new_thread_id(),
        new_turn_id(),
        Some(ModelRef::new("fake", "fake-tool-model")),
        ReasoningConfig::default(),
        30_000,
        Arc::new(EventEmitter::new(events.clone())),
        registry.context.clone(),
        Arc::new(HeadlessUserInputTransport),
        crate::test_model::no_context_preparation(),
        None,
    );
    let control: AgentControlConfig = serde_json::from_value(json!({
        "binary":input.binary,"max_parallel":10,"max_idle_processes":input.idle_cap,
        "roles":[{"name":"bench","description":"Synthetic local peer","config":input.config,
            "parallel_safe":true,"max_processes":10,"timeout_ms":120000,"max_summary_bytes":131072}]
    }))?;
    let owner = AgentControlRuntime::from_config(&control)?;
    let runner = owner.service().context("agent control not configured")?;
    events.mark("parent_ready", json!({}));
    let mut task_ids = vec![None; input.peers];
    for round in 0..input.rounds {
        *events.state.lock().unwrap() = Round {
            number: round,
            ..Round::default()
        };
        events.mark("round_start", json!({"round":round}));
        let mut waiting = tokio::task::JoinSet::new();
        for (peer, task_id) in task_ids.iter().enumerate() {
            let mut request = AgentControlRequest::new(
                "bench",
                "read_file payload.txt",
                AgentTask::new(
                    format!("synthetic peer {peer} round {round}"),
                    input.workspace.clone(),
                ),
            );
            if let Some(task_id) = task_id {
                request = request.with_metadata(json!({"task_id":task_id}));
            }
            let handle = runner.spawn(request, context.clone()).await?;
            let runner = runner.clone();
            waiting
                .spawn(async move { Ok::<_, anyhow::Error>((peer, runner.wait(&handle).await?)) });
        }
        while let Some(result) = waiting.join_next().await {
            let (peer, result) = result??;
            ensure!(
                result.status == AgentLifecycleStatus::Completed,
                "peer failed: {:?}: {}",
                result.status,
                result.summary
            );
            ensure!(
                result
                    .summary
                    .contains("Fake final answer after tool result"),
                "real model/tool round trip missing: {}",
                result.summary
            );
            ensure!(
                result.summary.contains("SYNTHETIC_PAYLOAD_"),
                "tool payload is missing from the model answer"
            );
            if input.idle_cap >= input.peers {
                ensure!(
                    result.metadata["resumable"] == true,
                    "resident peer unexpectedly evicted"
                );
            }
            task_ids[peer] = result.child_thread_id.map(|id| id.to_string());
            events.mark(
                "peer_finished",
                json!({"round":round,"peer":peer,"metadata":result.metadata}),
            );
        }
        {
            let state = events.state.lock().unwrap();
            ensure!(
                state.entered.len() == input.peers && state.concurrent,
                "peers did not overlap inside fake model"
            );
        }
        events.mark("round_settled", json!({"round":round}));
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    events.mark("resident_idle", json!({}));
    tokio::time::sleep(Duration::from_secs(1)).await;
    drop(runner);
    drop(owner);
    drop(context);
    drop(registry);
    events.mark("teardown", json!({}));
    tokio::time::sleep(Duration::from_secs(2)).await;
    events.mark("done", json!({}));
    Ok(())
}
