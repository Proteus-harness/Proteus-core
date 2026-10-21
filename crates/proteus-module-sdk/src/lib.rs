//! Experimental Rust SDK for out-of-process Proteus modules.
//!
//! This release implements component wire v3. [`tool::ToolExport`] adapts tool/v6;
//! the general [`Export`] surface keeps other slot implementations outside Core.
//! SDK versions are independent from wire/slot versions. Pin an exact release or
//! Git revision: alpha API compatibility is not promised across releases.
//!
//! The worker remains an executable with its configured OS permissions. Host
//! callbacks retain the active invocation's authority; SDK use grants no rights.

mod runtime;
mod transport;

pub mod hosts;
pub mod tool;

pub use proteus_contracts as contracts;
pub use proteus_module_protocol as protocol;
pub use runtime::{run, serve};

/// An implementation already bound to one explicitly requested slot/module id.
/// Construct it inside [`run`]'s factory; unsupported identities must fail there.
///
/// Concurrent invocations may call the same export. Shared state must therefore
/// be synchronized by the implementation. No direct cross-export calls are made.
pub trait Export: Send + Sync {
    fn manifest(&self) -> contracts::contracts::ProcessComponentExportManifest;

    fn invoke(
        &self,
        method: &str,
        params: serde_json::Value,
        host: &hosts::HostBridge,
    ) -> anyhow::Result<serde_json::Value>;
}
