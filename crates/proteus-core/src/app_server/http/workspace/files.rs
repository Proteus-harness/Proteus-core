//! Creating, renaming and deleting project entries for the client's project panel.
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use super::git::validate;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EntryKind {
    File,
    Directory,
}

#[derive(Serialize)]
pub(crate) struct Done {
    ok: bool,
}

const DONE: Done = Done { ok: true };

/// Creates an empty file or a directory, with any missing parents.
pub(crate) fn create(root: &Path, relative: &str, kind: EntryKind) -> Result<Done> {
    let target = leaf(root, relative)?;
    ensure!(!exists(&target)?, "path already exists: {relative}");
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    match kind {
        EntryKind::File => drop(
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)?,
        ),
        EntryKind::Directory => fs::create_dir(&target)?,
    }
    Ok(DONE)
}

/// Moves an entry; an existing target is never replaced.
pub(crate) fn rename(root: &Path, from: &str, to: &str) -> Result<Done> {
    let source = leaf(root, from)?;
    let target = leaf(root, to)?;
    ensure!(exists(&source)?, "path does not exist: {from}");
    ensure!(!exists(&target)?, "path already exists: {to}");
    ensure!(
        !target.starts_with(&source),
        "cannot move a directory into itself"
    );
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(source, target)?;
    Ok(DONE)
}

/// Deletes files, links and whole directories; links are never followed.
pub(crate) fn delete(root: &Path, paths: &[String]) -> Result<Done> {
    ensure!(!paths.is_empty(), "paths must not be empty");
    let targets = paths
        .iter()
        .map(|path| {
            let target = leaf(root, path)?;
            ensure!(exists(&target)?, "path does not exist: {path}");
            Ok(target)
        })
        .collect::<Result<Vec<_>>>()?;
    for target in targets {
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&target)?,
            Ok(_) => fs::remove_file(&target)?,
            // Already removed together with an earlier selected ancestor.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(DONE)
}

/// An entry strictly below the session root. Only its parent is resolved, so
/// the entry itself may be a link pointing anywhere.
fn leaf(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut names = Vec::new();
    for part in Path::new(relative).components() {
        match part {
            Component::Normal(name) => names.push(name.to_str().context("non-UTF-8 path")?),
            Component::CurDir => {}
            _ => bail!("workspace path must be relative and cannot contain '..'"),
        }
    }
    let name = names
        .pop()
        .context("a workspace path below the session root is required")?;
    let parent = if names.is_empty() {
        root.canonicalize()?
    } else {
        validate(root, &names.join("/"))?
    };
    Ok(parent.join(name))
}

fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
