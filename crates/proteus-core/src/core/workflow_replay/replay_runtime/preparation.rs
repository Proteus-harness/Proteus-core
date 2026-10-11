//! Preparation replays exact recorded hook proposals, including unchanged/errors.
use super::*;
use crate::contracts::{
    ContextPreparationInput, ContextPreparationOutput, ContextPreparationResult, ExecutionHooks,
    HookEvent, HookInput,
};

impl ReplayState {
    pub(super) async fn prepare(
        self: &std::sync::Arc<Self>,
        input: ContextPreparationInput,
        module_ids: Vec<String>,
    ) -> Result<ContextPreparationResult> {
        let trace = self
            .lock()
            .hooks
            .iter()
            .find(|(trace, consumed)| {
                !consumed
                    && trace.owner == OperationOwner::Workflow
                    && matches!(trace.input.event, HookEvent::BeforeContext { .. })
            })
            .map(|(trace, _)| trace.clone());
        let Some(trace) = trace else {
            if module_ids.is_empty() {
                return Ok(ContextPreparationResult {
                    output: ContextPreparationOutput::unchanged(input.messages),
                    target: self.lock().target.clone(),
                });
            }
            return mismatch(
                &mut self.lock(),
                "workflow requested unrecorded context preparation".into(),
            );
        };
        let HookEvent::BeforeContext { target, .. } = &trace.input.event else {
            unreachable!()
        };
        let event = HookEvent::BeforeContext {
            output: ContextPreparationOutput::unchanged(input.messages.clone()),
            input: input.clone(),
            target: self.lock().target.clone().or_else(|| target.clone()),
        };
        let result = ReplayHooks::new(self.clone(), module_ids)
            .apply(HookInput {
                event,
                ..trace.input
            })
            .await?;
        let HookEvent::BeforeContext { output, target, .. } = result else {
            unreachable!()
        };
        if output.changed {
            let report = HistoryCompactionReport::from_context_preparation_output(&input, &output);
            let mut inner = self.lock();
            if let Some(expected) = inner.compactions.iter_mut().find(|expected| {
                !expected.consumed
                    && super::super::normalize::changed_compactions_equal(
                        std::slice::from_ref(&report),
                        std::slice::from_ref(&expected.report),
                    )
            }) {
                expected.consumed = true;
            }
        }
        Ok(ContextPreparationResult { output, target })
    }
}
