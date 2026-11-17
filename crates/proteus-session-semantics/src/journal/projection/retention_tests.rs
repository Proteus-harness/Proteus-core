use super::super::types::ToolResultRecorded;
use super::*;
use proteus_contracts::domain::*;

#[test]
fn completed_tool_releases_effect_but_keeps_lifecycle_guards() {
    let execution_id = new_execution_id();
    let result = ToolResult::ok("call".into(), "large effect ".repeat(8192));
    let mut state = JournalValidationState::default();
    state.tool_calls.insert(
        result.call_id.clone(),
        Arc::new(ToolLifecycle {
            owner: Some(ExecutionFactOwner {
                execution_id,
                thread_id: None,
                turn_id: None,
            }),
            resolved: true,
            effect: Some(result.clone()),
            ..Default::default()
        }),
    );
    let mut record = JournalRecord {
        schema_version: JOURNAL_SCHEMA_VERSION,
        record_id: new_record_id(),
        session_seq: 1,
        timestamp_ms: 0,
        session_id: new_session_id(),
        execution_id: Some(execution_id),
        thread_id: None,
        turn_id: None,
        entry: JournalEntry::ToolResultRecorded(ToolResultRecorded {
            result: result.clone(),
        }),
    };
    let mut conflicting = record.clone();
    if let JournalEntry::ToolResultRecorded(tool) = &mut conflicting.entry {
        tool.result.ok = false;
    }
    assert!(state.clone().apply(&conflicting).is_err());
    state.apply(&record).unwrap();
    let lifecycle = state.tool_calls.get(&result.call_id).unwrap();
    assert!(lifecycle.result);
    assert!(lifecycle.effect.is_none());
    record.record_id = new_record_id();
    assert!(
        state
            .clone()
            .apply(&record)
            .unwrap_err()
            .to_string()
            .contains("duplicate tool result")
    );
    record.record_id = new_record_id();
    record.entry = JournalEntry::ToolEffectRecorded(ToolResultRecorded { result });
    assert!(
        state
            .apply(&record)
            .unwrap_err()
            .to_string()
            .contains("violates its call lifecycle")
    );
}
