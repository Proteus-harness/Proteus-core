use super::super::host_rpc::{callback_error, decode, encode};
use crate::{
    contracts::{
        CancellationToken, ConversationReader, ReadConversationRequest,
        SessionConversationSnapshot, TOOL_HOST_CONVERSATION_SNAPSHOT_METHOD,
        TOOL_HOST_READ_CONVERSATION_METHOD,
    },
    domain::ToolCall,
};
use proteus_module_protocol::{
    ProcessModuleRpcError,
    v3::{AsyncHostRequestDispatcher, ComponentHostRequest, HostRequestFuture},
};
use std::sync::Arc;

pub(super) struct ToolHost {
    pub terminal_host: Option<Arc<dyn crate::contracts::ToolTerminalHost>>,
    pub terminal_client: Arc<super::super::client::ProcessExportClient>,
    pub terminal_context: proteus_contracts::process_module::ToolModuleInvocationContext,
    pub terminal_ref: Arc<tokio::sync::Mutex<Option<crate::contracts::ToolTerminalRef>>>,
    pub operations: Option<Arc<dyn crate::contracts::ToolHost>>,
    pub conversation: Option<Arc<dyn ConversationReader>>,
    pub session_id: Option<crate::domain::SessionId>,
    pub call: ToolCall,
    pub cancellation: CancellationToken,
}

impl AsyncHostRequestDispatcher for ToolHost {
    fn dispatch(&self, request: ComponentHostRequest) -> HostRequestFuture {
        if request.method == crate::contracts::TOOL_HOST_PUBLISH_TERMINAL_METHOD {
            let host = self.terminal_host.clone();
            let client = self.terminal_client.clone();
            let context = self.terminal_context.clone();
            let terminal_ref = self.terminal_ref.clone();
            let cancellation = self.cancellation.clone();
            return Box::pin(async move {
                let descriptor: crate::contracts::ToolTerminalDescriptor =
                    decode(request.params, "publish_terminal")?;
                let result = async {
                    anyhow::ensure!(!cancellation.is_cancelled(), "tool invocation canceled");
                    let Some(host) = host else {
                        return Ok(serde_json::Value::Null);
                    };
                    let mut binding = terminal_ref.lock().await;
                    anyhow::ensure!(
                        binding.is_none(),
                        "terminal already published for this invocation"
                    );
                    let controller = Arc::new(TerminalController {
                        client,
                        context,
                        generation: request.invocation.generation(),
                        handle: descriptor.handle.clone(),
                    });
                    *binding = Some(host.publish(descriptor, controller).await?);
                    Ok(serde_json::Value::Null)
                }
                .await;
                result.map_err(|error| callback_error("publish_terminal", &error))
            });
        }
        if matches!(
            request.method.as_str(),
            crate::contracts::OPERATION_HOST_COMPLETE_MODEL_METHOD
                | crate::contracts::OPERATION_HOST_EXECUTE_TOOL_METHOD
        ) {
            return match &self.operations {
                Some(host) => super::super::operation_host::OperationHost::Tool(host.clone())
                    .dispatch(request),
                None => Box::pin(async {
                    Err(ProcessModuleRpcError::new(
                        -32100,
                        "tool operations are not bound",
                    ))
                }),
            };
        }
        let reader = self.conversation.clone();
        let call = self.call.clone();
        let cancellation = self.cancellation.clone();
        let session_id = self.session_id;
        Box::pin(async move {
            let method = request.method;
            if method != TOOL_HOST_READ_CONVERSATION_METHOD
                && method != TOOL_HOST_CONVERSATION_SNAPSHOT_METHOD
            {
                return Err(ProcessModuleRpcError::new(
                    -32601,
                    "unsupported tool host method",
                ));
            }
            let _: ReadConversationRequest = decode(request.params, &method)?;
            let result = async {
                anyhow::ensure!(!cancellation.is_cancelled(), "tool invocation canceled");
                let reader =
                    reader.ok_or_else(|| anyhow::anyhow!("tool invocation has no conversation"))?;
                let snapshot = reader.read().await?;
                let snapshot = if method == TOOL_HOST_READ_CONVERSATION_METHOD {
                    serde_json::to_value(snapshot.for_tool(&call)?)?
                } else {
                    serde_json::to_value(SessionConversationSnapshot {
                        session_id: session_id.ok_or_else(|| {
                            anyhow::anyhow!("tool invocation has no bound session")
                        })?,
                        conversation: snapshot,
                    })?
                };
                anyhow::ensure!(!cancellation.is_cancelled(), "tool invocation canceled");
                Ok(snapshot)
            }
            .await;
            encode(
                result.map_err(|error| callback_error(&method, &error))?,
                &method,
            )
        })
    }
}

struct TerminalController {
    client: Arc<super::super::client::ProcessExportClient>,
    context: proteus_contracts::process_module::ToolModuleInvocationContext,
    generation: u64,
    handle: String,
}
#[async_trait::async_trait]
impl crate::contracts::ToolTerminalController for TerminalController {
    async fn control(
        &self,
        action: crate::contracts::ToolTerminalAction,
    ) -> anyhow::Result<crate::contracts::ToolTerminalResponse> {
        self.client
            .invoke_for_generation(
                self.generation,
                crate::contracts::PROCESS_TOOL_TERMINAL_METHOD,
                &crate::contracts::ProcessToolTerminalInput {
                    request: crate::contracts::ToolTerminalRequest {
                        handle: self.handle.clone(),
                        action,
                    },
                    context: self.context.clone(),
                },
            )
            .await
    }
}
