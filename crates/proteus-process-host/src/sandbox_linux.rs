use std::{
    ffi::CString,
    fs::{self, File},
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt, fs::MetadataExt, process::CommandExt},
    },
    path::{Component, Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};

use super::ProcessSandbox;
use crate::ProcessSpec;

const BACKEND: &str = "/usr/bin/bwrap";

#[path = "sandbox_executable.rs"]
mod executable;

struct Grant {
    destination: PathBuf,
    source: PathBuf,
    fd: OwnedFd,
    writable: bool,
    directory: bool,
    identity: (u64, u64),
}

pub(super) fn prepare(spec: &ProcessSpec, sandbox: &ProcessSandbox) -> Result<Command> {
    let backend = fs::metadata(BACKEND).context("restricted sandbox requires /usr/bin/bwrap")?;
    ensure!(
        backend.is_file() && backend.mode() & 0o111 != 0,
        "restricted sandbox backend /usr/bin/bwrap is not executable"
    );
    let mut grants = Vec::<Grant>::new();
    for (paths, writable) in [(&sandbox.read_only, false), (&sandbox.read_write, true)] {
        for path in paths {
            let destination = normalized(path)?;
            check_reserved(&destination)?;
            let source = fs::canonicalize(&destination).with_context(|| {
                format!("sandbox grant does not exist: {}", destination.display())
            })?;
            check_reserved(&source)?;
            let fd = pin_path(&source)
                .with_context(|| format!("cannot pin sandbox grant {}", destination.display()))?;
            let metadata = File::from(fd.try_clone()?).metadata()?;
            ensure!(
                metadata.is_dir() || metadata.is_file(),
                "sandbox grants must be regular files or directories: {}",
                destination.display()
            );
            for prior in &grants {
                ensure!(
                    !overlaps(&destination, &prior.destination),
                    "overlapping sandbox destinations: {} and {}",
                    destination.display(),
                    prior.destination.display()
                );
                ensure!(
                    !(writable || prior.writable)
                        || (!overlaps(&source, &prior.source)
                            && (metadata.dev(), metadata.ino()) != prior.identity),
                    "overlapping sandbox source with writable authority: {} and {}",
                    destination.display(),
                    prior.destination.display()
                );
            }
            grants.push(Grant {
                destination,
                source,
                fd,
                writable,
                directory: metadata.is_dir(),
                identity: (metadata.dev(), metadata.ino()),
            });
        }
    }
    let environment = spec.resolved_environment()?;
    let requested_cwd = spec.cwd.clone().unwrap_or(std::env::current_dir()?);
    let cwd = if sandbox.read_executable {
        fs::canonicalize(&requested_cwd).context("cannot resolve sandbox cwd")?
    } else {
        normalized(&requested_cwd)?
    };
    ensure!(
        cwd.is_dir(),
        "sandbox cwd is not an existing directory: {}",
        cwd.display()
    );
    ensure!(
        grants
            .iter()
            .any(|grant| grant.directory && cwd.starts_with(&grant.destination)),
        "sandbox cwd must be reachable through an explicit directory grant: {}",
        cwd.display()
    );

    let executable = if sandbox.read_executable {
        Some(executable::grant(spec, &cwd, &environment, &mut grants)?)
    } else {
        None
    };
    let mut command = Command::new(BACKEND);
    // The backend itself must not receive caller-controlled loader variables.
    // Payload variables are installed only after namespace/mount setup.
    command.env_clear().args([
        "--unshare-all",
        "--unshare-user",
        "--disable-userns",
        "--cap-drop",
        "ALL",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
    ]);
    if sandbox.network {
        command.arg("--share-net");
    }
    for (key, value) in environment {
        command.arg("--setenv").arg(key).arg(value);
    }
    for grant in &grants {
        command
            .arg(if grant.writable {
                "--bind-fd"
            } else {
                "--ro-bind-fd"
            })
            .arg(grant.fd.as_raw_fd().to_string())
            .arg(&grant.destination);
    }
    command
        .args(["--remount-ro", "/"])
        .arg("--chdir")
        .arg(cwd)
        .arg("--")
        .arg(
            executable
                .as_deref()
                .unwrap_or_else(|| Path::new(&spec.command)),
        )
        .args(&spec.args);
    // Keep O_PATH descriptors alive until fork/exec, without changing their
    // parent-side CLOEXEC flags. bwrap verifies mounted inode/dev against each
    // supplied FD and closes consumed mount FDs before starting the payload.
    unsafe {
        command.pre_exec(move || {
            // Do not leak any unrelated inherited descriptors into either the
            // backend or payload. close_range is async-signal-safe; errors fail
            // the spawn instead of falling back to an unrestricted process.
            if libc::syscall(
                libc::SYS_close_range,
                3u32,
                u32::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            ) == -1
            {
                return Err(io::Error::last_os_error());
            }
            for grant in &grants {
                if libc::fcntl(grant.fd.as_raw_fd(), libc::F_SETFD, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    Ok(command)
}

fn normalized(path: &Path) -> Result<PathBuf> {
    ensure!(
        path.is_absolute(),
        "sandbox paths must be absolute: {}",
        path.display()
    );
    let mut result = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(name) => result.push(name),
            _ => bail!(
                "sandbox paths must not contain parent traversal: {}",
                path.display()
            ),
        }
    }
    Ok(result)
}

fn check_reserved(path: &Path) -> Result<()> {
    ensure!(
        path != Path::new("/")
            && path != Path::new("/tmp")
            && !["/proc", "/dev", "/sys", "/run"]
                .iter()
                .any(|root| path.starts_with(root)),
        "sandbox grant uses a reserved root: {}",
        path.display()
    );
    Ok(())
}

fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// Walk a canonical path relative to already pinned directory descriptors.
/// O_NOFOLLOW rejects ancestor replacement by symlinks; a rename of an opened
/// ancestor cannot redirect subsequent traversal to a different tree.
fn pin_path(path: &Path) -> Result<OwnedFd> {
    let root = CString::new("/")?;
    let mut fd = checked_fd(unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_PATH | libc::O_CLOEXEC | libc::O_DIRECTORY,
        )
    })?;
    let parts: Vec<_> = path
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    for (index, part) in parts.iter().enumerate() {
        let name = CString::new(part.as_bytes())?;
        let flags = libc::O_PATH
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | if index + 1 < parts.len() {
                libc::O_DIRECTORY
            } else {
                0
            };
        fd = checked_fd(unsafe { libc::openat(fd.as_raw_fd(), name.as_ptr(), flags) })?;
    }
    let metadata = File::from(fd.try_clone()?).metadata()?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "sandbox source changed to a symlink"
    );
    Ok(fd)
}

fn checked_fd(fd: i32) -> io::Result<OwnedFd> {
    if fd == -1 {
        Err(io::Error::last_os_error())
    } else {
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        if fd.as_raw_fd() >= 3 {
            Ok(fd)
        } else {
            // Stdio may have been closed by the caller. Mount descriptors must
            // never occupy slots subsequently replaced by Command's pipes.
            let duplicate = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
            if duplicate == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
            }
        }
    }
}

#[cfg(test)]
#[path = "sandbox_pin_tests.rs"]
mod tests;
