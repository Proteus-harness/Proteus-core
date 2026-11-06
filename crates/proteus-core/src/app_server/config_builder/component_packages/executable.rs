//! Resolve an existing alias with the environment and cwd its child receives.
use super::*;
use crate::process_adapters::ProcessComponentConfig;

pub(super) fn verify(
    component: &ProcessComponentConfig,
    expected: Option<&Path>,
    workspace: &Path,
) -> Result<()> {
    let command = Path::new(component.command());
    let Some(expected) = expected else {
        if !command.is_absolute() {
            anyhow::bail!(
                "component replacement requires expected_executable for a non-absolute command"
            );
        }
        return Ok(());
    };
    if !expected.is_absolute() {
        anyhow::bail!("component replacement expected_executable must be absolute");
    }
    let spec = component.process_spec(workspace)?;
    let environment = spec.resolved_environment()?;
    let cwd = spec.cwd.as_deref().unwrap_or(workspace);
    let candidate = if command.is_absolute() || component.command().contains('/') {
        let candidate = cwd.join(command);
        if !is_executable(&candidate) {
            anyhow::bail!(
                "component command is not a regular executable: {}",
                candidate.display()
            );
        }
        candidate
    } else {
        let path = environment
            .get("PATH")
            .context("component command requires child PATH")?;
        std::env::split_paths(path)
            .map(|entry| cwd.join(entry).join(command))
            .find(|candidate| is_executable(candidate))
            .context("component command not found in child PATH")?
    };
    let actual =
        std::fs::canonicalize(&candidate).context("cannot resolve component executable")?;
    let expected =
        std::fs::canonicalize(expected).context("cannot resolve expected component executable")?;
    if actual != expected {
        anyhow::bail!(
            "component executable changed since package selection: expected {}, resolved {}",
            expected.display(),
            actual.display()
        );
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::{
        ffi::CString,
        os::unix::{ffi::OsStrExt, fs::MetadataExt},
    };
    let regular = std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.mode() & 0o111 != 0);
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // Match execvp: a mode-executable PATH entry can still be inaccessible to
    // effective credentials (ACL/noexec), in which case search continues.
    regular
        && unsafe {
            libc::faccessat(libc::AT_FDCWD, path.as_ptr(), libc::X_OK, libc::AT_EACCESS) == 0
        }
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}
