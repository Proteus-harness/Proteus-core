use std::{path::PathBuf, process::Command};

use anyhow::Result;

use crate::ProcessSpec;

/// Explicit mounts for a restricted process generation.
///
/// No host filesystem roots or environment variables are implicitly granted.
/// `network = false` isolates IP networking. Unix sockets in granted trees are
/// still usable: this setting is not a general IPC firewall.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcessSandbox {
    pub read_only: Vec<PathBuf>,
    pub read_write: Vec<PathBuf>,
    pub network: bool,
}

#[cfg(target_os = "linux")]
#[path = "sandbox_linux.rs"]
mod linux;

pub(crate) fn prepare(spec: &ProcessSpec, sandbox: &ProcessSandbox) -> Result<Command> {
    #[cfg(target_os = "linux")]
    return linux::prepare(spec, sandbox);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (spec, sandbox);
        anyhow::bail!("restricted process sandbox is supported only on Linux");
    }
}
