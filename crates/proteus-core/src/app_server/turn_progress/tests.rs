use serde_json::json;

use super::*;
use crate::domain::{
    EventContext, SteeringDeliveryKind, ThreadId, ToolResult, new_session_id, new_thread_id,
    new_turn_id,
};

fn envelope(thread_id: ThreadId, event: Event) -> EventEnvelope {
    EventEnvelope::new(
        EventContext::new(new_session_id(), thread_id, Some(new_turn_id())),
        1,
        event,
    )
}

fn root_thread_id() -> ThreadId {
    ThreadId::parse_str("00000000-0000-0000-0000-000000000001").expect("root thread id")
}

fn child_thread_id() -> ThreadId {
    ThreadId::parse_str("00000000-0000-0000-0000-000000000002").expect("child thread id")
}

fn apply(progress: &mut TurnProgress, event: Event) {
    progress.apply(&envelope(root_thread_id(), event));
}

fn delta_for(message_id: u128, text: &str) -> Event {
    Event::AssistantTextDelta {
        offset: 0,
        message_id: uuid::Uuid::from_u128(message_id),
        phase: None,
        text: text.to_owned(),
    }
}

fn delta(text: &str) -> Event {
    delta_for(1, text)
}

#[test]
fn session_binding_updates_only_the_exact_existing_child_row() {
    let mut progress = TurnProgress::default();
    let child = child_thread_id();
    let other = new_thread_id();
    for id in [child, other] {
        apply(
            &mut progress,
            Event::SubagentStarted {
                role: "helper".into(),
                description: None,
                child_thread_id: id,
            },
        );
    }
    assert!(
        progress.messages.iter().all(|message| message
            .subagent
            .as_ref()
            .unwrap()
            .session
            .is_none())
    );
    let session_id = new_session_id();
    let session_dir = std::path::PathBuf::from("/sessions/exact-child");
    apply(
        &mut progress,
        Event::SubagentSessionBound {
            child_thread_id: child,
            session_id,
            session_dir: session_dir.clone(),
        },
    );
    assert_eq!(progress.messages.len(), 2);
    assert_eq!(
        progress.messages[0]
            .subagent
            .as_ref()
            .unwrap()
            .session
            .as_ref()
            .unwrap(),
        &proteus_contracts::app_protocol::AppSubagentSession {
            session_id,
            session_dir
        }
    );
    assert!(
        progress.messages[1]
            .subagent
            .as_ref()
            .unwrap()
            .session
            .is_none()
    );
}

#[test]
fn accumulates_text_segments_around_tool_calls() {
    let mut progress = TurnProgress::default();
    apply(&mut progress, delta("Сначала "));
    apply(&mut progress, delta("посмотрю файл."));
    apply(
        &mut progress,
        Event::ToolCallRequested {
            call: ToolCall::new("call-1", "read_file", json!({ "path": "src/lib.rs" })),
        },
    );
    apply(
        &mut progress,
        Event::ToolCallResolved {
            call: ToolCall::new("call-1", "read_file", json!({ "path": "src/effective.rs" })),
        },
    );
    apply(
        &mut progress,
        Event::ToolFinished {
            result: ToolResult::ok("call-1".to_owned(), "contents"),
        },
    );
    apply(&mut progress, delta_for(2, "Теперь answer."));

    let snapshot = progress.snapshot();
    assert_eq!(snapshot.len(), 3);
    assert_eq!(snapshot[0].text, "Сначала посмотрю файл.");
    assert!(!snapshot[0].streaming);
    let tool = snapshot[1].tool.as_ref().expect("tool entry");
    assert_eq!(tool.status, "done");
    assert_eq!(tool.result.as_deref(), Some("contents"));
    assert_eq!(tool.args["path"], "src/lib.rs");
    assert_eq!(
        tool.effective_args.as_ref().unwrap()["path"],
        "src/effective.rs"
    );
    // Последний текстовый сегмент — живой, клиент достримит в него.
    assert_eq!(snapshot[2].text, "Теперь answer.");
    assert!(snapshot[2].streaming);
}

#[test]
fn trailing_tool_is_not_marked_streaming() {
    let mut progress = TurnProgress::default();
    apply(&mut progress, delta("Запускаю."));
    apply(
        &mut progress,
        Event::ToolCallRequested {
            call: ToolCall::new("call-1", "shell", json!({})),
        },
    );

    let snapshot = progress.snapshot();
    assert_eq!(snapshot.len(), 2);
    assert!(!snapshot[1].streaming);
    assert_eq!(
        snapshot[1].tool.as_ref().map(|tool| tool.status.as_str()),
        Some("running")
    );
}

#[test]
fn child_thread_text_deltas_do_not_pollute_parent_progress() {
    let mut progress = TurnProgress::default();
    apply(
        &mut progress,
        Event::TurnStarted {
            session_id: new_session_id(),
            thread_id: root_thread_id(),
            turn_id: new_turn_id(),
        },
    );
    apply(&mut progress, delta("родительский текст"));
    // Стрим дочернего цикла под child thread не должен доклеиваться к
    // родительскому сегменту (и не должен создавать свой).
    progress.apply(&envelope(
        child_thread_id(),
        Event::AssistantTextDelta {
            offset: 0,
            message_id: proteus_contracts::domain::new_message_id(),
            phase: None,
            text: "детский стрим".to_owned(),
        },
    ));

    let snapshot = progress.snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].text, "родительский текст");
}

#[test]
fn turn_boundaries_clear_progress() {
    let mut progress = TurnProgress::default();
    apply(&mut progress, delta("старый ход"));
    apply(
        &mut progress,
        Event::TurnStarted {
            session_id: new_session_id(),
            thread_id: new_thread_id(),
            turn_id: new_turn_id(),
        },
    );
    assert!(progress.snapshot().is_empty());

    apply(&mut progress, delta("новый ход"));
    apply(
        &mut progress,
        Event::Error {
            message: "boom".to_owned(),
        },
    );
    assert_eq!(
        progress.snapshot()[0].text,
        "новый ход",
        "runtime error alone is not durable settlement"
    );
    progress.finish_parent_turn();
    assert!(progress.snapshot().is_empty());
}

#[test]
fn snapshots_subagent_card_and_nested_child_tools() {
    let mut progress = TurnProgress::default();
    let child_thread_id = child_thread_id();

    apply(
        &mut progress,
        Event::SubagentStarted {
            role: "reviewer".to_owned(),
            description: Some("check patch".to_owned()),
            child_thread_id,
        },
    );
    progress.apply(&envelope(
        child_thread_id,
        Event::ToolCallRequested {
            call: ToolCall::new("call-child", "read_file", json!({ "path": "src/lib.rs" })),
        },
    ));
    progress.apply(&envelope(
        child_thread_id,
        Event::ToolFinished {
            result: ToolResult::ok("call-child".to_owned(), "contents"),
        },
    ));
    apply(
        &mut progress,
        Event::SubagentFinished {
            role: "reviewer".to_owned(),
            status: "completed".to_owned(),
            iterations: 2,
            child_thread_id,
        },
    );

    let snapshot = progress.snapshot();
    assert_eq!(snapshot.len(), 1);
    assert!(!snapshot[0].streaming);
    assert!(snapshot[0].tool.is_none());
    let subagent = snapshot[0].subagent.as_ref().expect("subagent");
    assert_eq!(subagent.role, "reviewer");
    assert_eq!(subagent.description.as_deref(), Some("check patch"));
    assert_eq!(subagent.child_thread_id, child_thread_id.to_string());
    assert_eq!(subagent.status, "completed");
    assert_eq!(subagent.iterations, Some(2));
    assert_eq!(subagent.tools.len(), 1);
    assert_eq!(subagent.tools[0].call_id, "call-child");
    assert_eq!(subagent.tools[0].status, "done");
    assert_eq!(subagent.tools[0].result.as_deref(), Some("contents"));

    let long_result = "a".repeat(MAX_SUBAGENT_TOOL_RESULT_BYTES - 1) + &"я".repeat(100_000);
    progress.apply(&envelope(
        child_thread_id,
        Event::ToolFinished {
            result: ToolResult::ok("call-child".to_owned(), long_result.clone()),
        },
    ));
    // Inspect the retained string, not snapshot()'s length-sized clone.
    let retained = progress.messages[0].subagent.as_ref().unwrap().tools[0]
        .result
        .as_ref()
        .unwrap();
    assert_eq!(retained, &long_result[..MAX_SUBAGENT_TOOL_RESULT_BYTES - 1]);
    assert!(retained.capacity() < long_result.len());
}

#[test]
fn tool_finish_borrows_direct_text_before_truncating_at_a_scalar_boundary() {
    let call_id = "bounded-preview";
    let error_prefix = "e".repeat(MAX_SUBAGENT_TOOL_RESULT_BYTES - 1);
    let output_prefix = "o".repeat(MAX_SUBAGENT_TOOL_RESULT_BYTES - 2);
    let tail = "z".repeat(256 * 1024);
    let output = format!("{output_prefix}🦀{tail}");
    let content = vec![crate::domain::ToolContent::Text {
        text: "structured fallback".to_owned(),
    }];
    for (error, output, expected) in [
        (
            Some(format!("{error_prefix}🦀{tail}")),
            output.clone(),
            error_prefix.as_str(),
        ),
        (Some(String::new()), output.clone(), output_prefix.as_str()),
        (Some(" ".to_owned()), output, " "),
    ] {
        let result = ToolResult::new(
            call_id.to_owned(),
            false,
            output,
            content.clone(),
            error,
            serde_json::Value::Null,
        );
        let source = result
            .error
            .as_deref()
            .filter(|text| !text.is_empty())
            .unwrap_or(&result.output);
        let text = tool_result_text(&result);
        assert!(matches!(&text, Cow::Borrowed(_)));
        assert_eq!(text.as_ptr(), source.as_ptr());
        assert_eq!(
            truncate_utf8(&result.text_or_status(), MAX_SUBAGENT_TOOL_RESULT_BYTES),
            expected,
        );
        drop(text);

        let mut progress = TurnProgress::default();
        apply(
            &mut progress,
            Event::ToolCallRequested {
                call: ToolCall::new(call_id, "synthetic", json!({})),
            },
        );
        apply(&mut progress, Event::ToolFinished { result });
        let tool = progress.messages[0].tool.as_ref().unwrap();
        assert_eq!(tool.status, "failed");
        assert_eq!(tool.result.as_deref(), Some(expected));
    }
    let structured = ToolResult::ok(call_id.to_owned(), "").with_content(content);
    assert_eq!(
        tool_result_text(&structured).as_ref(),
        structured.text_or_status(),
    );
}

#[test]
fn preview_prefix_respects_utf8_byte_boundaries() {
    for (text, limit, expected) in [
        ("", 0, ""),
        ("я😀z", 0, ""),
        ("я😀z", 1, ""),
        ("я😀z", 2, "я"),
        ("я😀z", 5, "я"),
        ("я😀z", 6, "я😀"),
        ("я😀z", 7, "я😀z"),
        ("я😀z", 8, "я😀z"),
    ] {
        assert_eq!(truncate_utf8(text, limit), expected);
    }
}

#[test]
fn compact_tool_json_keeps_the_escaped_prefix_or_the_original_value() {
    let value = json!({"nested": [null, "я😀\n\""]});
    let encoded = value.to_string();
    for limit in 0..=encoded.len() + 1 {
        let expected = if limit < encoded.len() {
            json!({"truncated": true, "preview": truncate_utf8(&encoded, limit)})
        } else {
            value.clone()
        };
        assert_eq!(compact_json(&value, limit), expected, "limit={limit}");
    }
}

#[test]
fn collaboration_subagent_survives_parent_turn_and_keeps_late_tools_nested() {
    let mut progress = TurnProgress::default();
    let child_thread_id = child_thread_id();

    apply(
        &mut progress,
        Event::ToolCallRequested {
            call: ToolCall::new(
                "spawn-1",
                "spawn_agent",
                json!({
                    "task_name": "scan",
                    "message": "inspect",
                    "agent_type": "explore"
                }),
            ),
        },
    );
    apply(
        &mut progress,
        Event::SubagentStarted {
            role: "explore".to_owned(),
            description: Some("scan".to_owned()),
            child_thread_id,
        },
    );
    apply(
        &mut progress,
        Event::ToolFinished {
            result: ToolResult::ok("spawn-1".to_owned(), "started"),
        },
    );
    apply(
        &mut progress,
        Event::TurnFinished {
            output: crate::domain::AgentOutput::text("spawned"),
        },
    );
    progress.finish_parent_turn();

    progress.apply(&envelope(
        child_thread_id,
        Event::ToolCallRequested {
            call: ToolCall::new("child-1", "read_file", json!({ "path": "src/lib.rs" })),
        },
    ));
    progress.apply(&envelope(
        child_thread_id,
        Event::ToolFinished {
            result: ToolResult::ok("child-1".to_owned(), "contents"),
        },
    ));

    let snapshot = progress.snapshot();
    assert_eq!(
        snapshot.len(),
        1,
        "late child tool must not become flat progress"
    );
    let subagent = snapshot[0].subagent.as_ref().expect("background subagent");
    assert_eq!(subagent.status, "running");
    assert_eq!(subagent.tools.len(), 1);
    assert_eq!(subagent.tools[0].status, "done");

    apply(
        &mut progress,
        Event::TurnStarted {
            session_id: new_session_id(),
            thread_id: root_thread_id(),
            turn_id: new_turn_id(),
        },
    );
    apply(&mut progress, delta("new parent turn"));
    let snapshot = progress.snapshot();
    assert_eq!(snapshot.len(), 2, "next turn retains card");
    assert!(
        snapshot[0].streaming,
        "background card must not hide stream tail"
    );
    assert!(snapshot[1].subagent.is_some());

    apply(
        &mut progress,
        Event::SubagentFinished {
            role: "explore".to_owned(),
            status: "completed".to_owned(),
            iterations: 2,
            child_thread_id,
        },
    );
    let snapshot = progress.snapshot();
    assert_eq!(
        snapshot[1]
            .subagent
            .as_ref()
            .map(|subagent| subagent.status.as_str()),
        Some("completed")
    );
}

#[test]
fn collaboration_followup_owns_a_background_card() {
    let mut progress = TurnProgress::default();
    let child_thread_id = child_thread_id();

    apply(
        &mut progress,
        Event::ToolCallRequested {
            call: ToolCall::new(
                "followup-1",
                "followup_task",
                json!({ "target": "/root/scan", "message": "continue" }),
            ),
        },
    );
    apply(
        &mut progress,
        Event::SubagentStarted {
            role: "explore".to_owned(),
            description: Some("scan".to_owned()),
            child_thread_id,
        },
    );
    apply(
        &mut progress,
        Event::ToolFinished {
            result: ToolResult::ok("followup-1".to_owned(), "resumed"),
        },
    );
    apply(
        &mut progress,
        Event::TurnFinished {
            output: crate::domain::AgentOutput::text("follow-up started"),
        },
    );
    progress.finish_parent_turn();
    progress.apply(&envelope(
        child_thread_id,
        Event::ToolCallRequested {
            call: ToolCall::new("child-late", "grep", json!({ "pattern": "mailbox" })),
        },
    ));

    let snapshot = progress.snapshot();
    assert_eq!(snapshot.len(), 1);
    let subagent = snapshot[0].subagent.as_ref().expect("follow-up child");
    assert_eq!(subagent.status, "running");
    assert_eq!(subagent.tools.len(), 1);
    assert_eq!(subagent.tools[0].call_id, "child-late");
}

#[test]
fn steering_user_message_splits_assistant_stream_segments() {
    let mut progress = TurnProgress::default();
    apply(
        &mut progress,
        Event::TurnStarted {
            session_id: new_session_id(),
            thread_id: root_thread_id(),
            turn_id: new_turn_id(),
        },
    );
    apply(&mut progress, delta("before steering"));
    apply(
        &mut progress,
        Event::SteeringDelivered {
            message_id: crate::domain::new_message_id(),
            text: "change direction".to_owned(),
            images: vec![crate::domain::ImageRef {
                id: "queued-image".to_owned(),
                name: "queued.png".to_owned(),
                mime_type: "image/png".to_owned(),
                path: "images/queued.png".into(),
            }],
            kind: SteeringDeliveryKind::Steering,
            queued_count: 0,
        },
    );
    apply(&mut progress, delta_for(2, "after steering"));

    let snapshot = progress.snapshot();
    assert_eq!(snapshot.len(), 3);
    assert_eq!(snapshot[0].role, "assistant");
    assert_eq!(snapshot[0].text, "before steering");
    assert_eq!(snapshot[1].role, "user");
    assert_eq!(snapshot[1].text, "change direction");
    assert_eq!(snapshot[1].images[0].id, "queued-image");
    assert!(snapshot[1].message_id.is_some());
    assert_eq!(snapshot[2].role, "assistant");
    assert_eq!(snapshot[2].text, "after steering");
    assert!(snapshot[2].streaming);
}

#[test]
fn followup_images_are_visible_after_turn_started_before_settlement() {
    let mut progress = TurnProgress::default();
    let turn_id = new_turn_id();
    let message_id = crate::domain::new_message_id();
    apply(
        &mut progress,
        Event::TurnStarted {
            session_id: new_session_id(),
            thread_id: root_thread_id(),
            turn_id,
        },
    );
    apply(
        &mut progress,
        Event::SteeringDelivered {
            message_id,
            text: String::new(),
            kind: SteeringDeliveryKind::FollowUp,
            queued_count: 0,
            images: vec![crate::domain::ImageRef {
                id: "followup-image".into(),
                name: "followup.png".into(),
                mime_type: "image/png".into(),
                path: "images/followup.png".into(),
            }],
        },
    );
    let snapshot = progress.snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].message_id, Some(message_id));
    assert_eq!(snapshot[0].images[0].id, "followup-image");
}

#[test]
fn creation_dates_do_not_follow_text_deltas_completion_or_tool_finish() {
    let mut progress = TurnProgress::default();
    let events = [
        delta("first"),
        delta(" second"),
        Event::AssistantMessageCompleted {
            message_id: uuid::Uuid::from_u128(1),
            phase: None,
            text: "complete".into(),
        },
        Event::ToolCallRequested {
            call: ToolCall::new("dated", "probe", json!({})),
        },
        Event::ToolFinished {
            result: ToolResult::ok("dated".into(), "done"),
        },
    ];
    for (index, event) in events.into_iter().enumerate() {
        let mut item = envelope(root_thread_id(), event);
        item.timestamp_ms = 10 + index as i64;
        progress.apply(&item);
    }
    let rows = progress.snapshot();
    assert_eq!(rows[0].timestamp_ms, Some(10));
    assert_eq!(rows[0].text, "complete");
    assert_eq!(rows[1].timestamp_ms, Some(13));
    assert_eq!(rows[1].tool.as_ref().unwrap().status, "done");
}

#[test]
fn terminal_button_is_bound_before_tool_result_and_survives_final_metadata() {
    let mut progress = TurnProgress::default();
    let call =
        crate::domain::ToolCall::new("terminal-call", "exec_command", json!({"cmd":"synthetic"}));
    apply(
        &mut progress,
        Event::ToolCallRequested { call: call.clone() },
    );
    let terminal = crate::contracts::ToolTerminalRef {
        id: uuid::Uuid::new_v4(),
        title: "synthetic".into(),
        tty: true,
    };
    apply(
        &mut progress,
        Event::ToolTerminalBound {
            call_id: call.id.clone(),
            terminal: terminal.clone(),
        },
    );
    assert_eq!(
        progress.messages[0]
            .tool
            .as_ref()
            .unwrap()
            .terminal
            .as_ref(),
        Some(&terminal)
    );
    let mut result = ToolResult::ok(call.id, "finished");
    result.metadata = json!({"tool_terminal":terminal});
    apply(&mut progress, Event::ToolFinished { result });
    let tool = progress.messages[0].tool.as_ref().unwrap();
    assert_eq!(tool.status, "done");
    assert_eq!(tool.terminal, Some(terminal));
}

#[path = "tests/reasoning.rs"]
mod reasoning;
