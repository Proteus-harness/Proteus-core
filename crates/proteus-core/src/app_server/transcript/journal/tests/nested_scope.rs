use super::*;
use crate::contracts::OperationOwner;

#[test]
fn nested_display_calls_remain_bound_to_their_root_thread_and_turn() {
    let session = new_session_id();
    let execution = new_execution_id();
    let thread = new_thread_id();
    let turn = new_turn_id();
    let other_thread = new_thread_id();
    let other_turn = new_turn_id();
    let opened = || {
        JournalEntry::TurnOpened(TurnOpened {
            intent: None,
            task: AgentTask::new("scope", "/workspace".into()),
            base_history_revision: 0,
            module_epoch: 0,
            config_snapshot: json!({}),
        })
    };
    let requested = |id: &str| {
        JournalEntry::ToolCallRecorded(ToolCallRecorded {
            call: ToolCall::new(id, "shell", json!({})),
            phase: ToolCallRecordPhase::Requested {
                owner: OperationOwner::Tool {
                    call_id: "parent".into(),
                },
            },
        })
    };
    let result = |id: &str, text: &str| {
        JournalEntry::ToolResultRecorded(ToolResultRecorded {
            result: ToolResult::ok(id.into(), text),
        })
    };
    let mut records = vec![
        record(session, Some(execution), thread, turn, 1, opened()),
        record(
            session,
            Some(execution),
            thread,
            turn,
            2,
            requested("child"),
        ),
        record(
            session,
            Some(execution),
            thread,
            turn,
            3,
            result("child", "correct child output"),
        ),
        record(
            session,
            Some(execution),
            other_thread,
            turn,
            4,
            requested("wrong-thread"),
        ),
        record(
            session,
            Some(execution),
            other_thread,
            turn,
            5,
            result("wrong-thread", "hidden"),
        ),
        record(
            session,
            Some(execution),
            thread,
            other_turn,
            6,
            requested("unopened-turn"),
        ),
        record(
            session,
            Some(execution),
            thread,
            other_turn,
            7,
            result("unopened-turn", "hidden"),
        ),
        record(
            session,
            Some(new_execution_id()),
            other_thread,
            other_turn,
            8,
            opened(),
        ),
        // A display-eligible call in another root turn must not grant this
        // otherwise unrequested result permission to mutate its old row.
        record(
            session,
            Some(new_execution_id()),
            other_thread,
            other_turn,
            9,
            result("child", "wrong turn must not overwrite"),
        ),
        record(
            session,
            Some(execution),
            thread,
            turn,
            10,
            requested("detached"),
        ),
        record(
            session,
            Some(execution),
            thread,
            turn,
            11,
            result("detached", "hidden"),
        ),
    ];
    for record in &mut records[9..] {
        record.thread_id = None;
        record.turn_id = None;
    }
    // Renderer-only scope fixture: validate both the streaming and collected
    // projection without pretending its deliberately malformed lifecycles are
    // a valid canonical journal.
    let rows = journal_transcript_messages(records.clone(), None);
    let tools: Vec<_> = rows.iter().filter_map(|row| row.tool.as_ref()).collect();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].call_id, "child");
    assert_eq!(tools[0].result.as_deref(), Some("correct child output"));
    let live = journal_transcript_messages(records, Some(turn));
    assert!(live.is_empty(), "an active root turn remains event-owned");
}
