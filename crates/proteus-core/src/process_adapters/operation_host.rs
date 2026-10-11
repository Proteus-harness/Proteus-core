//! Shared canonical JSON callbacks. Slot invocation hosts retain their authority.
use super::host_rpc::{decode, encode};
use crate::contracts::{
    HookHost, OPERATION_HOST_COMPLETE_MODEL_METHOD, OPERATION_HOST_EXECUTE_TOOL_METHOD,
    OperationCompleteModelRequest, OperationExecuteToolRequest, ToolHost,
};
use proteus_module_protocol::{
    ProcessModuleRpcError,
    v3::{AsyncHostRequestDispatcher, ComponentHostRequest, HostRequestFuture},
};
use std::sync::Arc;
#[derive(Clone)]
pub(super) enum OperationHost {
    Hook(Arc<dyn HookHost>),
    Tool(Arc<dyn ToolHost>),
}
impl AsyncHostRequestDispatcher for OperationHost {
    fn dispatch(&self, request: ComponentHostRequest) -> HostRequestFuture {
        let host = self.clone();
        Box::pin(async move {
            let method = request.method;
            match method.as_str() {
                OPERATION_HOST_COMPLETE_MODEL_METHOD => {
                    let input: OperationCompleteModelRequest = decode(request.params, &method)?;
                    let result = match host {
                        Self::Hook(host) => host.complete_model(input.request).await,
                        Self::Tool(host) => host.complete_model(input.request).await,
                    };
                    encode(
                        result.map_err(|error| callback_error(&method, &error))?,
                        &method,
                    )
                }
                OPERATION_HOST_EXECUTE_TOOL_METHOD => {
                    let input: OperationExecuteToolRequest = decode(request.params, &method)?;
                    let result = match host {
                        Self::Hook(host) => host.execute_tool(input.call).await,
                        Self::Tool(host) => host.execute_tool(input.call).await,
                    };
                    encode(
                        result.map_err(|error| callback_error(&method, &error))?,
                        &method,
                    )
                }
                _ => Err(ProcessModuleRpcError::new(
                    -32601,
                    "unsupported extension host method",
                )),
            }
        })
    }
}

fn callback_error(method: &str, error: &anyhow::Error) -> ProcessModuleRpcError {
    let rpc = super::host_rpc::callback_error(method, error);
    match error.downcast_ref::<crate::model_standard::ModelFailure>() {
        Some(failure) => rpc.with_data(
            serde_json::to_value(failure).expect("canonical model failure serialization"),
        ),
        None => rpc,
    }
}
