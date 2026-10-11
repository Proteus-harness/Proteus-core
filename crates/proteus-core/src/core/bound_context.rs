//! Context building bound to one execution and its durable recorder.

use std::{sync::Arc, time::Duration};

use anyhow::{Result, anyhow, ensure};
use async_trait::async_trait;

use crate::{
    contracts::{
        ContextBuildInput, ContextBuildOutcome, ContextBuilder, ExecutionAttribution,
        ExecutionRecorder, ExecutionScope,
    },
    domain::ContextBundle,
};

pub(crate) struct BoundContext {
    inner: Arc<dyn ContextBuilder>,
    scope: ExecutionScope,
    attribution: ExecutionAttribution,
    recorder: Arc<dyn ExecutionRecorder>,
    timeout_ms: u64,
}

impl BoundContext {
    pub(crate) fn new(
        inner: Arc<dyn ContextBuilder>,
        scope: ExecutionScope,
        attribution: ExecutionAttribution,
        recorder: Arc<dyn ExecutionRecorder>,
        timeout_ms: u64,
    ) -> Self {
        Self {
            inner,
            scope,
            attribution,
            recorder,
            timeout_ms,
        }
    }
}

#[async_trait]
impl ContextBuilder for BoundContext {
    async fn build(&self, input: ContextBuildInput) -> Result<ContextBundle> {
        ensure!(
            input.scope.execution_id == self.scope.execution_id,
            "context build execution identity does not match its binding"
        );
        ensure!(
            input.attribution == self.attribution,
            "context build attribution does not match its binding"
        );
        let invocation_id = uuid::Uuid::new_v4();
        self.recorder
            .context_request_recorded(invocation_id, &input.task)
            .await?;

        // A per-call child token may narrow cancellation, but cannot bypass
        // the cancellation boundary established when this builder was bound.
        let call_cancellation = input.scope.cancellation.clone();
        let result = if self.scope.cancellation.is_cancelled() || call_cancellation.is_cancelled() {
            Err(anyhow!("turn canceled by client"))
        } else {
            tokio::select! {
                biased;
                _ = self.scope.cancellation.cancelled() => Err(anyhow!("turn canceled by client")),
                _ = call_cancellation.cancelled() => Err(anyhow!("turn canceled by client")),
                result = tokio::time::timeout(
                    Duration::from_millis(self.timeout_ms),
                    self.inner.build(input),
                ) => match result {
                    Ok(result) => result,
                    Err(_) => Err(anyhow!("context build timed out after {}ms", self.timeout_ms)),
                },
            }
        };
        let outcome = match &result {
            Ok(bundle) => ContextBuildOutcome::Success {
                bundle: bundle.clone(),
            },
            Err(error) => ContextBuildOutcome::Error {
                message: error.to_string(),
                causes: error.chain().skip(1).map(ToString::to_string).collect(),
            },
        };
        self.recorder
            .context_response_recorded(invocation_id, &outcome)
            .await?;
        result
    }
}

#[cfg(test)]
mod tests;
