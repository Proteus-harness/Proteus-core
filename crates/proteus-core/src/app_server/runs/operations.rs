//! Detached session tools retain their own cancellation and settlement without
//! replacing the root turn or restricting the runtime's existing concurrency.
use super::*;

const MAX_STANDALONE_OPERATIONS: usize = 32;

impl AppServerHandle {
    pub(in crate::app_server) async fn run_tool_call(
        &self,
        id: Option<String>,
        call: crate::domain::ToolCall,
    ) -> Result<crate::domain::ToolResult> {
        let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let cancellation = CancellationToken::new();
        let (settled_tx, settled) = watch::channel(false);
        {
            let mut runs = self.runs.lock().await;
            anyhow::ensure!(!runs.closed, "session is shutting down");
            anyhow::ensure!(
                runs.operations.len() < MAX_STANDALONE_OPERATIONS,
                "session standalone operation limit reached ({MAX_STANDALONE_OPERATIONS})"
            );
            anyhow::ensure!(
                !runs.operations.contains_key(&id)
                    && !runs
                        .active
                        .as_ref()
                        .is_some_and(|run| run.info.run_id == id),
                "run id is already active: {id}"
            );
            runs.operations.insert(
                id.clone(),
                StandaloneOperation {
                    cancellation: cancellation.clone(),
                    settled,
                },
            );
        }
        let server = self.clone();
        // The session registry owns cancellation and a settlement receiver even
        // when the transport drops this response consumer.
        let operation = tokio::spawn(async move {
            let result = server.runtime.execute_tool(call, cancellation).await;
            server.runs.lock().await.operations.remove(&id);
            settled_tx.send_replace(true);
            result
        });
        operation.await?
    }
}
