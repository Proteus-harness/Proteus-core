//! Bounded Git views and actions on the live session workspace.
mod command;
mod diff;
mod history;
mod status;
mod write;

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Result, bail, ensure};

pub(crate) use diff::diff;
pub(crate) use history::{branches, log};
pub(crate) use status::changes;
pub(crate) use write::{Selection, checkout, commit, discard, stage, unstage};

use command::{os, read, run};

fn repository(root: &Path) -> Result<bool> {
    let result = run(root, &["rev-parse", "--is-inside-work-tree"])?;
    Ok(result.success && result.bytes == b"true\n")
}

fn require_repository(root: &Path) -> Result<()> {
    ensure!(
        repository(root)?,
        "session workspace is not a Git repository"
    );
    Ok(())
}

fn has_head(root: &Path) -> Result<bool> {
    Ok(run(root, &["rev-parse", "--verify", "--quiet", "HEAD"])?.success)
}

/// Location of the session cwd inside the repository, with a trailing `/`.
fn prefix(root: &Path) -> Result<String> {
    let prefix = run(root, &["rev-parse", "--show-prefix"])?;
    ensure!(
        prefix.success && !prefix.truncated,
        "cannot resolve Git workspace prefix"
    );
    Ok(std::str::from_utf8(&prefix.bytes)?
        .strip_suffix('\n')
        .unwrap_or_default()
        .to_owned())
}

/// Files of the workspace that Git does not ignore, relative to `root`:
/// tracked and untracked ones. `None` outside a repository; the flag
/// reports a listing cut at `limit` bytes.
pub(super) fn visible_files(root: &Path, limit: usize) -> Result<Option<(Vec<String>, bool)>> {
    if !repository(root)? {
        return Ok(None);
    }
    let output = read(
        root,
        &os(&[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--deduplicate",
            "--",
            ".",
        ]),
        limit,
    )?;
    ensure!(
        output.success || output.truncated,
        "cannot list workspace files"
    );
    let files = complete_records(&output.bytes)
        .filter_map(|path| std::str::from_utf8(path).ok().map(str::to_owned))
        .collect();
    Ok(Some((files, output.truncated)))
}

/// NUL-terminated records; an incomplete final record cut at the cap is dropped.
fn complete_records(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let complete = bytes.iter().rposition(|b| *b == 0).map_or(0, |i| i + 1);
    bytes[..complete]
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
}

// Deleted files have no canonical leaf. Check every existing ancestor and reject
// lexical traversal before Git receives the original, literal workspace path.
fn validate(root: &Path, relative: &str) -> Result<PathBuf> {
    ensure!(!relative.is_empty(), "a workspace path is required");
    let root = root.canonicalize()?;
    let mut path = root.clone();
    for part in Path::new(relative).components() {
        match part {
            Component::Normal(name) => path.push(name),
            Component::CurDir => continue,
            _ => bail!("workspace path must be relative and cannot contain '..'"),
        }
        match fs::symlink_metadata(&path) {
            Ok(_) => ensure!(
                path.canonicalize()?.starts_with(&root),
                "path is outside the session workspace"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    ensure!(
        path != root,
        "a workspace path below the session root is required"
    );
    Ok(path)
}

/// A validated path in the normalized form passed to Git as a literal pathspec.
fn pathspec(root: &Path, relative: &str) -> Result<String> {
    validate(root, relative)?;
    Ok(Path::new(relative)
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/"))
}
