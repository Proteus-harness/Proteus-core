//! The source facade and its original parent attribution survive process setup;
//! recording failure is not converted into a live-only, non-durable link.
use super::*;
use proteus_contracts::{
    contracts::{ExecutionAttribution, OperationOwner, ToolExecutionRecorder},
    domain::{SubagentSessionBinding, ToolCall, ToolCallResolution, ToolResult},
};

#[derive(Default)]
struct BindingRecorder {
    bindings: std::sync::Mutex<Vec<(ExecutionAttribution, SubagentSessionBinding)>>,
    fail: bool,
}

#[async_trait::async_trait]
impl ToolExecutionRecorder for BindingRecorder {
    async fn subagent_session_bound(
        &self,
        attribution: ExecutionAttribution,
        binding: &SubagentSessionBinding,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(!self.fail, "binding recorder failure");
        self.bindings
            .lock()
            .unwrap()
            .push((attribution, binding.clone()));
        Ok(())
    }
    async fn tool_call_requested(
        &self,
        _: ExecutionAttribution,
        _: OperationOwner,
        _: &ToolCall,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    async fn tool_call_resolved(
        &self,
        _: ExecutionAttribution,
        _: &ToolCall,
        _: &ToolCallResolution,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    async fn tool_approval_requested(
        &self,
        _: ExecutionAttribution,
        _: &ToolCall,
        _: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    async fn tool_result_recorded(
        &self,
        _: ExecutionAttribution,
        _: &ToolResult,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn durable_binding_records_explicit_facade_and_original_parent_attribution() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let config = write_child_config(home.path());
    let runner = process_runner(&config);
    let events = Arc::new(InMemoryEventStore::new());
    let mut ctx = test_runtime_context(events.clone());
    let recorder = Arc::new(BindingRecorder::default());
    ctx.tool_recorder = recorder.clone();
    let expected = ExecutionAttribution::for_turn(
        ctx.execution.scope.execution_id,
        ctx.session_id,
        ctx.thread_id,
        ctx.turn_id,
    );
    let task = AgentTask::new("parent", workspace.path().into());
    let first = runner
        .run(
            AgentControlRequest::new("helper", "first", task.clone())
                .with_metadata(json!({"source_call_id":"spawn-call"})),
            ctx.clone(),
        )
        .await
        .unwrap();
    let child = first.child_thread_id.unwrap();
    let second = runner
        .run(
            AgentControlRequest::new("helper", "resume", task)
                .with_metadata(json!({"source_call_id":"followup-call","task_id":child})),
            ctx,
        )
        .await
        .unwrap();
    let recorded = recorder.bindings.lock().unwrap();
    assert_eq!(
        recorded.len(),
        2,
        "one durable fact for each facade generation"
    );
    assert_eq!(recorded[0].0, expected);
    assert_eq!(recorded[1].0, expected);
    assert_eq!(recorded[0].1.source_call_id, "spawn-call");
    assert_eq!(recorded[1].1.source_call_id, "followup-call");
    assert_eq!(recorded[0].1.child_thread_id, child);
    assert_eq!(recorded[1].1.child_thread_id, child);
    assert_eq!(recorded[0].1.session_id, recorded[1].1.session_id);
    assert_eq!(recorded[0].1.session_dir, recorded[1].1.session_dir);
    assert_eq!(
        json!(recorded[0].1.session_id),
        first.metadata["subagent_session"]["session_id"]
    );
    assert_eq!(
        first.metadata["subagent_session"],
        second.metadata["subagent_session"]
    );
}

#[tokio::test]
async fn recording_failure_invalidates_peer_before_publishing_a_session_link() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let config = write_child_config(home.path());
    let runner = process_runner(&config);
    let events = Arc::new(InMemoryEventStore::new());
    let mut ctx = test_runtime_context(events.clone());
    ctx.tool_recorder = Arc::new(BindingRecorder {
        fail: true,
        ..Default::default()
    });
    let error = runner
        .run(
            AgentControlRequest::new(
                "helper",
                "fail binding",
                AgentTask::new("parent", workspace.path().into()),
            )
            .with_metadata(json!({"source_call_id":"spawn-call"})),
            ctx,
        )
        .await
        .expect_err("canonical recording failure must be visible");
    assert!(format!("{error:#}").contains("binding recorder failure"));
    let events = events.envelopes().await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.event, Event::SubagentSessionBound { .. }))
    );
    assert!(events.iter().any(|event| matches!(&event.event, Event::SubagentFinished { status, .. } if status == "errored")));
}
