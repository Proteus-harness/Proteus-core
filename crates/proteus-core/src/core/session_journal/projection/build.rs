//! Build a strict read projection and hand the same validation state to a writer.
use super::*;

impl JournalProjection {
    pub fn build(session_id: SessionId, records: Vec<JournalRecord>) -> Result<Self> {
        let state = validate_records(session_id, &records)?;
        Ok(Self::from_validated(records, state))
    }

    pub(crate) fn from_validated(
        records: Vec<JournalRecord>,
        state: JournalValidationState,
    ) -> Self {
        let mut interrupted_model_exchanges = state
            .model_requests
            .keys()
            .filter(|exchange_id| !state.model_responses.contains(exchange_id))
            .copied()
            .collect::<Vec<_>>();
        interrupted_model_exchanges.sort_unstable();
        let mut unresolved_tool_calls = state
            .tool_calls
            .iter()
            .filter(|(_, lifecycle)| !lifecycle.result && lifecycle.effect.is_none())
            .map(|(call_id, _)| call_id.clone())
            .collect::<Vec<_>>();
        unresolved_tool_calls.sort();
        let mut unsettled_turns = state
            .opened_turns
            .keys()
            .filter(|turn_id| !state.settled_turns.contains(turn_id))
            .copied()
            .collect::<Vec<_>>();
        unsettled_turns.sort_unstable();

        let interrupted_context_builds = state.context.interrupted();

        Self {
            records,
            history: Arc::unwrap_or_clone(state.history),
            history_revision: state.history_revision,
            interrupted_turns: Arc::unwrap_or_clone(state.interrupted_turns),
            interrupted_model_exchanges,
            interrupted_context_builds,
            unresolved_tool_calls,
            unsettled_turns,
        }
    }
}

fn validate_records(
    session_id: SessionId,
    records: &[JournalRecord],
) -> Result<JournalValidationState> {
    let mut state = JournalValidationState::default();
    let mut expected_seq = 1_u64;
    for record in records {
        state.apply_sequenced(session_id, record, &mut expected_seq)?;
    }
    Ok(state)
}
