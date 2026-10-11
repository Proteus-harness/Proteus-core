use super::{ProcessExportClient, ProcessExportConfig};
use crate::contracts::{
    HookHandler, HookHost, HookInput, HookResponse, PROCESS_HOOK_CONTRACT_VERSION,
    PROCESS_HOOK_INVOKE_METHOD, ProcessHookResponse,
};
use anyhow::Result;
use async_trait::async_trait;

use std::{path::Path, sync::Arc};

pub struct ProcessHookAdapter {
    client: Arc<ProcessExportClient>,
}
impl ProcessHookAdapter {
    pub fn new(
        config: ProcessExportConfig,
        workspace: &Path,
        workflow_timeout_ms: u64,
    ) -> Result<Self> {
        let timeout_ms = if workflow_timeout_ms == 0 {
            30_000
        } else {
            workflow_timeout_ms.saturating_add(1_000)
        };
        Ok(Self {
            client: Arc::new(ProcessExportClient::connect(
                "hook",
                PROCESS_HOOK_CONTRACT_VERSION,
                config,
                workspace,
                timeout_ms,
            )?),
        })
    }
}
#[async_trait]
impl HookHandler for ProcessHookAdapter {
    async fn invoke(&self, input: HookInput, host: Arc<dyn HookHost>) -> Result<HookResponse> {
        let response: ProcessHookResponse = self
            .client
            .invoke_with_dispatcher_and_cancel_check(
                PROCESS_HOOK_INVOKE_METHOD,
                &input,
                Arc::new(super::operation_host::OperationHost::Hook(host.clone())),
                move || host.is_cancelled(),
            )
            .await
            .map_err(decode_hook_failure)?;
        Ok(response.result)
    }
}

fn decode_hook_failure(error: anyhow::Error) -> anyhow::Error {
    let rpc = error
        .downcast_ref::<proteus_module_protocol::ProcessModuleRpcError>()
        .or_else(|| {
            match error
                .downcast_ref::<super::ProcessInvocationError>()?
                .failure()
            {
                super::ProcessInvocationFailure::Module(rpc) => Some(rpc),
                _ => None,
            }
        });
    let Some(rpc) = rpc else {
        return error;
    };
    let Some(data) = rpc.data.clone() else {
        return error;
    };
    match serde_json::from_value::<crate::model_standard::ModelFailure>(data) {
        Ok(failure) => failure.into(),
        Err(invalid) => anyhow::anyhow!("invalid hook model failure: {invalid}"),
    }
}
