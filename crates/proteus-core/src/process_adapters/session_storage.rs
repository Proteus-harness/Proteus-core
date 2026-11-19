//! Callback-free persistence service selected by the host configuration.
use std::{path::Path, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;

use crate::contracts::{
    PROCESS_SESSION_STORAGE_CONTRACT_VERSION, PROCESS_SESSION_STORAGE_EXECUTE_METHOD,
    SESSION_STORAGE_DISPLAY_PROJECTION_FEATURE, SessionStorage, SessionStorageFailure,
    SessionStorageRequest, SessionStorageResponse,
};

use super::{
    ProcessExportClient, ProcessExportConfig, ProcessInvocationError, ProcessInvocationFailure,
};

const DEFAULT_TIMEOUT_MS: u64 = 30_000;

pub struct ProcessSessionStorage {
    client: Arc<ProcessExportClient>,
}

impl ProcessSessionStorage {
    pub fn new(config: ProcessExportConfig, workspace: &Path) -> Result<Self> {
        Ok(Self {
            client: Arc::new(ProcessExportClient::connect(
                "session_storage",
                PROCESS_SESSION_STORAGE_CONTRACT_VERSION,
                config,
                workspace,
                DEFAULT_TIMEOUT_MS,
            )?),
        })
    }
}

#[async_trait]
impl SessionStorage for ProcessSessionStorage {
    fn supports_display_projection(&self) -> bool {
        self.client
            .supports_feature(SESSION_STORAGE_DISPLAY_PROJECTION_FEATURE)
    }

    async fn execute(&self, request: SessionStorageRequest) -> Result<SessionStorageResponse> {
        request.validate()?;
        let response = self
            .client
            .invoke(PROCESS_SESSION_STORAGE_EXECUTE_METHOD, &request)
            .await
            .map_err(storage_failure)?;
        request.validate_response(&response)?;
        Ok(response)
    }

    fn execute_blocking(&self, request: SessionStorageRequest) -> Result<SessionStorageResponse> {
        request.validate()?;
        let response = self
            .client
            .invoke_blocking(PROCESS_SESSION_STORAGE_EXECUTE_METHOD, &request)
            .map_err(storage_failure)?;
        request.validate_response(&response)?;
        Ok(response)
    }
}

fn storage_failure(error: anyhow::Error) -> anyhow::Error {
    if let Some(ProcessInvocationFailure::Module(rpc)) = error
        .downcast_ref::<ProcessInvocationError>()
        .map(ProcessInvocationError::failure)
        && let Some(data) = &rpc.data
        && let Ok(failure) = serde_json::from_value::<SessionStorageFailure>(data.clone())
    {
        return error.context(failure);
    }
    error
}

#[cfg(test)]
mod tests;
