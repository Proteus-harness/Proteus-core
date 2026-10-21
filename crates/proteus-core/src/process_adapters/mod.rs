//! Adapters that expose host-configured stdio processes as module slots.

mod client;
mod config;
mod context;
mod hook;
mod host_rpc;
mod invocation_error;
mod invocation_scope;
mod model;
mod policy;
mod sandbox;
mod tool;
mod workflow;

pub use config::{ProcessComponentConfig, ProcessExportLaunchConfig};

pub(crate) use client::*;
pub(crate) use config::{ProcessComponentLauncher, ProcessExportConfig};
pub(crate) use context::*;
pub(crate) use hook::*;
pub(crate) use invocation_error::*;
pub(crate) use model::*;
pub(crate) use policy::*;
pub(crate) use tool::*;
pub(crate) use workflow::*;

mod operation_host;
