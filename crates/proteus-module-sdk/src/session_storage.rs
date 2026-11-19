//! Typed, callback-free session_storage/v2 export.
use anyhow::{Result, bail};
use proteus_contracts::contracts::{
    PROCESS_SESSION_STORAGE_CONTRACT_VERSION, PROCESS_SESSION_STORAGE_EXECUTE_METHOD,
    ProcessComponentExportInitialize, ProcessComponentExportManifest, ProcessModuleComposition,
    SESSION_STORAGE_DISPLAY_PROJECTION_FEATURE, SessionStorageRequest, SessionStorageResponse,
};
use serde_json::Value;

use crate::{Export, hosts::HostBridge};

/// Persistence owns completion of an admitted durable transaction. Its stable
/// operation id allows an ambiguous transport result to be reconciled later.
/// No execution/model/tool callbacks are exposed to this implementation.
pub trait SessionStorageModule: Send + Sync {
    fn supports_display_projection(&self) -> bool {
        false
    }

    fn execute(&self, request: SessionStorageRequest) -> Result<SessionStorageResponse>;
}

pub struct SessionStorageExport<T> {
    binding: ProcessComponentExportInitialize,
    storage: T,
}

impl<T: SessionStorageModule> SessionStorageExport<T> {
    pub fn new(binding: ProcessComponentExportInitialize, storage: T) -> Result<Self> {
        if binding.slot != "session_storage"
            || binding.contract_version != PROCESS_SESSION_STORAGE_CONTRACT_VERSION
            || binding.composition != ProcessModuleComposition::SelectOne
        {
            bail!(
                "SessionStorageExport requires session_storage/{PROCESS_SESSION_STORAGE_CONTRACT_VERSION} select_one"
            );
        }
        Ok(Self { binding, storage })
    }

    fn display_projection_enabled(&self) -> bool {
        self.storage.supports_display_projection()
            && self
                .binding
                .host_features
                .iter()
                .any(|feature| feature == SESSION_STORAGE_DISPLAY_PROJECTION_FEATURE)
    }
}

impl<T: SessionStorageModule> Export for SessionStorageExport<T> {
    fn manifest(&self) -> ProcessComponentExportManifest {
        ProcessComponentExportManifest {
            slot: self.binding.slot.clone(),
            module_id: self.binding.module_id.clone(),
            contract_version: self.binding.contract_version.clone(),
            composition: self.binding.composition,
            module_features: if self.display_projection_enabled() {
                vec![SESSION_STORAGE_DISPLAY_PROJECTION_FEATURE.into()]
            } else {
                Vec::new()
            },
            config_schema: None,
        }
    }

    fn invoke(&self, method: &str, params: Value, _host: &HostBridge) -> Result<Value> {
        if method != PROCESS_SESSION_STORAGE_EXECUTE_METHOD {
            bail!("unsupported session storage method: {method}");
        }
        let request: SessionStorageRequest = serde_json::from_value(params)?;
        if matches!(
            request,
            SessionStorageRequest::OpenDisplaySnapshot { .. }
                | SessionStorageRequest::ReadDisplayRecords { .. }
        ) && !self.display_projection_enabled()
        {
            bail!("session storage display projection was not negotiated");
        }
        request.validate()?;
        Ok(serde_json::to_value(self.storage.execute(request)?)?)
    }
}
