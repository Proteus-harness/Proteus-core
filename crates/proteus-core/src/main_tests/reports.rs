use super::*;

#[test]
fn eval_report_output_contains_core_metrics() {
    let report = proteus_core::core::EvalReport {
        journal_path: PathBuf::from(".proteus/sessions/1234567890/journal.jsonl"),
        records: 9,
        turns_started: 1,
        turns_finished: 1,
        turns_failed: 0,
        model_calls: 2,
        tool_calls: 3,
        tool_failures: 1,
        approvals_requested: 1,
        approvals_resolved: 1,
        approvals_approved: 0,
        approvals_denied: 1,
        estimated_input_tokens: 100,
        provider_input_tokens: 90,
        provider_output_tokens: 30,
        changed_files: vec!["src/lib.rs".to_owned()],
        duration_ms: Some(42),
        failure_reason: None,
    };

    let rendered = render_eval_report(&report);

    assert!(rendered.contains("Status: success"));
    assert!(rendered.contains("Turns: started=1, finished=1, failed=0"));
    assert!(rendered.contains("tool calls: 3 (failures=1)"));
    assert!(rendered.contains("provider_output=30"));
    assert!(rendered.contains("Changed files: src/lib.rs"));
}

#[test]
fn prompt_replay_human_and_json_reports_contain_key_fields() {
    use proteus_contracts::{
        domain::{
            ModelRef, new_exchange_id, new_execution_id, new_session_id, new_thread_id, new_turn_id,
        },
        model_standard::TokenUsage,
    };
    use proteus_core::core::{
        PROMPT_REPLAY_REPORT_SCHEMA_VERSION, PromptReplayCounts, PromptReplayNames,
        PromptReplayOutcomeStatus, PromptReplayOutcomeSummary, PromptReplayReport,
        PromptReplaySource, PromptReplayUsage,
    };

    let report = PromptReplayReport {
        schema_version: PROMPT_REPLAY_REPORT_SCHEMA_VERSION,
        source: PromptReplaySource {
            journal_path: PathBuf::from("/tmp/session/1234567890/journal.jsonl"),
            session_id: new_session_id(),
            execution_id: new_execution_id(),
            thread_id: Some(new_thread_id()),
            turn_id: Some(new_turn_id()),
            exchange_id: new_exchange_id(),
        },
        recorded_model: ModelRef::new("openai", "gpt-recorded"),
        replay_model: ModelRef::new("openai", "gpt-recorded"),
        replay_adapter: "openai.responses".to_owned(),
        recorded_outcome: PromptReplayOutcomeSummary {
            status: PromptReplayOutcomeStatus::Response,
            finish_reason: Some("stop".to_owned()),
            error: None,
            text: Some("recorded".to_owned()),
        },
        replay_outcome: PromptReplayOutcomeSummary {
            status: PromptReplayOutcomeStatus::Response,
            finish_reason: Some("tool_calls".to_owned()),
            error: None,
            text: Some("replayed".to_owned()),
        },
        usage: PromptReplayUsage {
            recorded: Some(TokenUsage::new(100, 20)),
            replay: Some(TokenUsage::new(110, 25)),
        },
        text_equal: Some(false),
        local_tool_calls: PromptReplayCounts {
            recorded: 0,
            replay: 1,
        },
        local_tool_call_names: PromptReplayNames {
            recorded: Vec::new(),
            replay: vec!["read_file".to_owned()],
        },
        hosted_activities: PromptReplayCounts {
            recorded: 0,
            replay: 0,
        },
        citations: PromptReplayCounts {
            recorded: 0,
            replay: 0,
        },
        request_hosted_tools: Vec::new(),
        hosted_tools_allowed: false,
        duration_ms: 42,
    };

    let human =
        cli_prompt_replay::render_prompt_replay_report(&report, false).expect("human report");
    assert!(human.contains("Prompt replay report"));
    assert!(human.contains(&format!("Session: {}", report.source.session_id)));
    assert!(human.contains(&format!("Execution: {}", report.source.execution_id)));
    assert!(human.contains(&format!("Exchange: {}", report.source.exchange_id)));
    assert!(human.contains("Recorded model: openai/gpt-recorded"));
    assert!(human.contains("Replay outcome: response (finish_reason=tool_calls)"));
    assert!(human.contains("Local tool calls: recorded=0, replay=1"));
    assert!(human.contains("Replay local tool calls (not executed): read_file"));
    assert!(human.contains("Duration: 42 ms"));

    let rendered_json =
        cli_prompt_replay::render_prompt_replay_report(&report, true).expect("JSON report");
    let value: serde_json::Value = serde_json::from_str(&rendered_json).expect("parse JSON");
    assert_eq!(value["schema_version"], 2);
    assert_eq!(
        value["source"]["exchange_id"],
        report.source.exchange_id.to_string()
    );
    assert_eq!(value["recorded_model"]["model"], "gpt-recorded");
    assert_eq!(value["replay_outcome"]["status"], "response");
    assert_eq!(value["local_tool_calls"]["replay"], 1);
    assert_eq!(value["duration_ms"], 42);
}

#[test]
fn workflow_replay_human_and_json_reports_contain_key_fields() {
    use proteus_contracts::domain::{AgentOutput, new_session_id, new_thread_id, new_turn_id};
    use proteus_core::core::{
        TurnSettlementStatus, WORKFLOW_REPLAY_REPORT_SCHEMA_VERSION, WorkflowReplayComparison,
        WorkflowReplayCounts, WorkflowReplayOutcome, WorkflowReplayReport, WorkflowReplaySource,
    };

    let report = WorkflowReplayReport {
        schema_version: WORKFLOW_REPLAY_REPORT_SCHEMA_VERSION,
        source: WorkflowReplaySource {
            journal_path: PathBuf::from("/tmp/session/1234567890/journal.jsonl"),
            session_id: new_session_id(),
            thread_id: new_thread_id(),
            turn_id: new_turn_id(),
            module_epoch: 4,
            profile_name: "codex".to_owned(),
            workflow_id: "coding.codex_loop".to_owned(),
            policy_id: "codex_policy".to_owned(),
        },
        recorded: WorkflowReplayOutcome {
            status: TurnSettlementStatus::Success,
            output: Some(AgentOutput::text("done")),
            error: None,
        },
        replay: WorkflowReplayOutcome {
            status: TurnSettlementStatus::Success,
            output: Some(AgentOutput::text("done")),
            error: None,
        },
        model_exchanges: WorkflowReplayCounts {
            recorded: 2,
            replayed: 2,
        },
        tool_calls: WorkflowReplayCounts {
            recorded: 1,
            replayed: 1,
        },
        comparison: WorkflowReplayComparison {
            matched: true,
            settlement_equal: true,
            output_equal: Some(true),
            error_equal: None,
            history_equal: Some(true),
            issues: Vec::new(),
        },
        source_journal_unchanged: true,
        duration_ms: 9,
    };

    let human =
        cli_workflow_replay::render_workflow_replay_report(&report, false).expect("human report");
    assert!(human.contains("Workflow replay report"));
    assert!(human.contains("Workflow: coding.codex_loop (policy=codex_policy, epoch=4)"));
    assert!(human.contains("Model exchanges: recorded=2, replayed=2"));
    assert!(human.contains("Tool calls: recorded=1, replayed=1"));
    assert!(human.contains("Status: matched"));
    assert!(human.contains("Replay text:\ndone"));

    let rendered_json =
        cli_workflow_replay::render_workflow_replay_report(&report, true).expect("JSON report");
    let value: serde_json::Value = serde_json::from_str(&rendered_json).expect("parse JSON");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["source"]["workflow_id"], "coding.codex_loop");
    assert_eq!(value["comparison"]["matched"], true);
    assert_eq!(value["tool_calls"]["replayed"], 1);
    assert_eq!(value["duration_ms"], 9);
}
