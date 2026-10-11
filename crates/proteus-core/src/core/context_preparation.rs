//! Prepares a candidate through the ordered hooks. Persistence stays with the workflow checkpoint.
use crate::contracts::{
    ContextPreparation, ContextPreparationInput, ContextPreparationOutput,
    ContextPreparationResult, ContextPreparationTarget, ConversationReader, ExecutionAttribution,
    ExecutionContext, HookEvent, HookInput, validate_context_preparation_output,
};
use anyhow::{Result, ensure};
use async_trait::async_trait;
use std::{path::PathBuf, sync::Arc};

pub(crate) struct RuntimeContextPreparation {
    execution: ExecutionContext,
    attribution: ExecutionAttribution,
    cwd: PathBuf,
    conversation: Option<Arc<dyn ConversationReader>>,
}
impl RuntimeContextPreparation {
    pub(crate) fn new(
        execution: ExecutionContext,
        attribution: ExecutionAttribution,
        cwd: PathBuf,
        conversation: Option<Arc<dyn ConversationReader>>,
    ) -> Self {
        Self {
            execution,
            attribution,
            cwd,
            conversation,
        }
    }
}
#[async_trait]
impl ContextPreparation for RuntimeContextPreparation {
    async fn prepare(&self, input: ContextPreparationInput) -> Result<ContextPreparationResult> {
        ensure!(
            !self.execution.is_cancelled(),
            "context preparation canceled"
        );
        let conversation = match &self.conversation {
            Some(reader) => Some(reader.read().await?),
            None => None,
        };
        let target = conversation
            .as_ref()
            .map(|snapshot| ContextPreparationTarget {
                execution_id: self.execution.scope.execution_id,
                revision: snapshot.revision,
            });
        let event = self
            .execution
            .hooks
            .apply(HookInput {
                event: HookEvent::BeforeContext {
                    output: ContextPreparationOutput::unchanged(input.messages.clone()),
                    input: input.clone(),
                    target: target.clone(),
                },
                attribution: self.attribution,
                cwd: self.cwd.clone(),
                conversation,
            })
            .await?;
        let HookEvent::BeforeContext {
            input: accepted_input,
            output,
            target: accepted_target,
        } = event
        else {
            anyhow::bail!("context preparation hook returned a different event");
        };
        ensure!(
            accepted_input == input && accepted_target == target,
            "context preparation changed bound input or target"
        );
        validate_context_preparation_output(&input, &output)?;
        Ok(ContextPreparationResult { output, target })
    }
}
