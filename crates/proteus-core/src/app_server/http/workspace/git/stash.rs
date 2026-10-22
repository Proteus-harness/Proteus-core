//! Stashing local changes and undoing the last commit.
use std::path::Path;

use anyhow::{Result, ensure};

use super::{
    command::{os, run, write},
    require_repository,
    status::{Changes, changes},
};

/// Stashes the changes under the session cwd, untracked files included unless
/// only the index is stashed.
pub(crate) fn stash(root: &Path, message: Option<&str>, staged_only: bool) -> Result<Changes> {
    require_repository(root)?;
    let before = stash_top(root)?;
    let mut args = vec!["stash", "push", "--quiet"];
    if staged_only {
        args.push("--staged");
    } else {
        args.push("--include-untracked");
    }
    let message = message.map(str::trim).filter(|message| !message.is_empty());
    if let Some(message) = message {
        args.extend(["--message", message]);
    }
    args.extend(["--", "."]);
    write(root, &os(&args))?.checked("stash")?;
    ensure!(stash_top(root)? != before, "no local changes to stash");
    changes(root)
}

pub(crate) fn stash_pop(root: &Path) -> Result<Changes> {
    require_repository(root)?;
    write(root, &os(&["stash", "pop", "--quiet"]))?.checked("stash pop")?;
    changes(root)
}

/// `git reset --soft HEAD~1`: the last commit's changes return to the index.
pub(crate) fn uncommit(root: &Path) -> Result<Changes> {
    require_repository(root)?;
    let parent = run(root, &["rev-parse", "--verify", "--quiet", "HEAD~1"])?;
    ensure!(parent.success, "HEAD has no parent commit to return to");
    write(root, &os(&["reset", "--soft", "--quiet", "HEAD~1"]))?.checked("reset")?;
    changes(root)
}

fn stash_top(root: &Path) -> Result<Option<Vec<u8>>> {
    let output = run(root, &["rev-parse", "--verify", "--quiet", "refs/stash"])?;
    Ok(output.success.then_some(output.bytes))
}
