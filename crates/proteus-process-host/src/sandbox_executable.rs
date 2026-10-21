use std::{collections::BTreeMap, ffi::OsString};

use super::*;

/// Resolve once against the exact payload environment, then mount the pinned
/// regular file at that logical absolute path. No PATH lookup occurs in bwrap.
pub(super) fn grant(
    spec: &ProcessSpec,
    cwd: &Path,
    environment: &BTreeMap<String, OsString>,
    grants: &mut Vec<Grant>,
) -> Result<PathBuf> {
    let command = Path::new(&spec.command);
    ensure!(
        !spec.command.is_empty(),
        "sandbox executable command is empty"
    );
    let candidate = if command.is_absolute() || spec.command.contains('/') {
        let path = cwd.join(command);
        ensure!(
            is_executable(&path),
            "sandbox command is not a regular executable: {}",
            path.display()
        );
        path
    } else {
        let path = environment
            .get("PATH")
            .context("sandbox executable resolution requires child PATH")?;
        std::env::split_paths(path)
            .map(|entry| cwd.join(entry).join(command))
            .find(|candidate| is_executable(candidate))
            .with_context(|| {
                format!(
                    "sandbox executable not found in child PATH: {}",
                    spec.command
                )
            })?
    };
    let destination = lexical_absolute(&candidate);
    check_reserved(&destination)?;
    let source = fs::canonicalize(&candidate).context("cannot resolve sandbox executable")?;
    check_reserved(&source)?;
    let fd = pin_path(&source).context("cannot pin sandbox executable")?;
    let metadata = File::from(fd.try_clone()?).metadata()?;
    ensure!(
        metadata.is_file() && metadata.mode() & 0o111 != 0,
        "sandbox executable changed to a non-executable file"
    );
    let identity = (metadata.dev(), metadata.ino());
    for prior in grants.iter() {
        ensure!(
            !prior.writable
                || (!overlaps(&destination, &prior.destination)
                    && !overlaps(&source, &prior.source)
                    && identity != prior.identity),
            "sandbox executable overlaps writable authority: {} and {}",
            destination.display(),
            prior.destination.display()
        );
        if overlaps(&destination, &prior.destination) {
            ensure!(
                !prior.writable
                    && ((prior.directory && destination.starts_with(&prior.destination))
                        || (destination == prior.destination && identity == prior.identity)),
                "sandbox executable overlaps an incompatible grant: {} and {}",
                destination.display(),
                prior.destination.display()
            );
        }
    }
    // Deliberately appended after explicit read-only directory grants: the
    // executable inode stays pinned even if that directory entry is replaced.
    grants.push(Grant {
        destination: destination.clone(),
        source,
        fd,
        writable: false,
        directory: false,
        identity,
    });
    Ok(destination)
}

fn is_executable(path: &Path) -> bool {
    let regular =
        fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.mode() & 0o111 != 0);
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // Effective credentials and ACL/noexec permissions matter too: execvp
    // skips EACCES candidates rather than stopping at their mode bits.
    regular
        && unsafe {
            libc::faccessat(libc::AT_FDCWD, path.as_ptr(), libc::X_OK, libc::AT_EACCESS) == 0
        }
}

/// PATH entries and slash-relative commands follow the child's canonical cwd.
/// Unlike user grants these command paths may contain normal parent traversal.
fn lexical_absolute(path: &Path) -> PathBuf {
    let mut result = PathBuf::from("/");
    for part in path.components() {
        match part {
            Component::Normal(name) => result.push(name),
            Component::ParentDir => {
                result.pop();
            }
            _ => {}
        }
    }
    result
}

#[cfg(test)]
#[path = "sandbox_executable_tests.rs"]
mod tests;
