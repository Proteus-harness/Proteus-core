//! The single hardened Git launcher shared by workspace reads and writes.
use std::{
    ffi::OsStr,
    io::{self, Read},
    path::Path,
    process::{Child, Command, Stdio},
    thread,
};

use anyhow::{Context, Result, bail};

use super::super::MAX_BYTES;

const MAX_STDERR_BYTES: usize = 4 * 1024;
const MAX_ERROR_CHARS: usize = 2000;

pub(super) struct Output {
    pub(super) success: bool,
    pub(super) different: bool,
    pub(super) bytes: Vec<u8>,
    pub(super) truncated: bool,
    stderr: Vec<u8>,
}

impl Output {
    /// Turns a failed invocation into Git's own bounded diagnostic.
    pub(super) fn checked(self, action: &str) -> Result<Self> {
        if self.success {
            return Ok(self);
        }
        let stderr = String::from_utf8_lossy(&self.stderr);
        let stdout = String::from_utf8_lossy(&self.bytes);
        let message = [stderr.trim(), stdout.trim()]
            .into_iter()
            .find(|text| !text.is_empty())
            .map(|text| text.chars().take(MAX_ERROR_CHARS).collect::<String>())
            .unwrap_or_else(|| format!("git {action} failed"));
        bail!("{message}")
    }
}

fn command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(root).args([
        "--no-pager",
        "--literal-pathspecs",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "status.renames=true",
        "-c",
        "core.editor=true",
    ]);
    // The session cwd, rather than the server's inherited Git environment, owns scope.
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
    ] {
        command.env_remove(name);
    }
    // Diagnostics are returned to clients verbatim, so keep them untranslated.
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_EDITOR", "true")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LANGUAGE", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Read-only invocation: stdout beyond `limit` stops Git and marks truncation.
pub(super) fn read(root: &Path, args: &[&OsStr], limit: usize) -> Result<Output> {
    collect(command(root).args(args), limit, true)
}

/// Mutating invocation: Git always runs to completion; excess stdout is drained.
pub(super) fn write(root: &Path, args: &[&OsStr]) -> Result<Output> {
    collect(command(root).args(args), MAX_BYTES, false)
}

pub(super) fn run(root: &Path, args: &[&str]) -> Result<Output> {
    read(root, &os(args), MAX_BYTES)
}

pub(super) fn os<'a>(args: &'a [&'a str]) -> Vec<&'a OsStr> {
    args.iter().map(OsStr::new).collect()
}

fn collect(command: &mut Command, limit: usize, stop_on_overflow: bool) -> Result<Output> {
    let mut child = command.spawn().context("cannot start Git")?;
    let errors = drain_stderr(&mut child);
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut bytes = Vec::new();
    let mut read = (&mut stdout)
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map(drop);
    let truncated = bytes.len() > limit;
    if truncated && !stop_on_overflow && read.is_ok() {
        read = io::copy(&mut stdout, &mut io::sink()).map(drop);
    }
    if (truncated && stop_on_overflow) || read.is_err() {
        let _ = child.kill();
    }
    drop(stdout);
    let status = child.wait()?;
    let stderr = errors.join().unwrap_or_default();
    read?;
    bytes.truncate(limit);
    Ok(Output {
        success: status.success(),
        different: status.code() == Some(1),
        bytes,
        truncated,
        stderr,
    })
}

fn drain_stderr(child: &mut Child) -> thread::JoinHandle<Vec<u8>> {
    let mut stderr = child.stderr.take().expect("piped stderr");
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = (&mut stderr)
            .take(MAX_STDERR_BYTES as u64)
            .read_to_end(&mut bytes);
        let _ = io::copy(&mut stderr, &mut io::sink());
        bytes
    })
}
