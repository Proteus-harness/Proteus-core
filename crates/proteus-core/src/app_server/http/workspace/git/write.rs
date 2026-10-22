//! Index, worktree and branch changes requested by the client's Git panel.
use std::{
    ffi::OsStr,
    fs,
    path::{Component, Path},
};

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use super::{
    command::{os, run, write},
    has_head,
    history::{CommitSummary, head_commit},
    pathspec, require_repository,
    status::{Change, Changes, changes, scan},
    validate,
};

/// Paths of the session workspace an action applies to.
pub(crate) enum Selection {
    All,
    Paths(Vec<String>),
}

impl Selection {
    pub(crate) fn new(paths: Vec<String>, all: bool) -> Result<Self> {
        match (all, paths.is_empty()) {
            (true, true) => Ok(Self::All),
            (true, false) => bail!("paths must be empty when all is true"),
            (false, true) => bail!("paths must not be empty unless all is true"),
            (false, false) => Ok(Self::Paths(paths)),
        }
    }

    fn pathspecs(&self, root: &Path) -> Result<Vec<String>> {
        match self {
            Self::All => Ok(vec![".".to_owned()]),
            Self::Paths(paths) => paths.iter().map(|path| pathspec(root, path)).collect(),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct Committed {
    commit: CommitSummary,
    changes: Changes,
}

pub(crate) fn stage(root: &Path, selection: Selection) -> Result<Changes> {
    require_repository(root)?;
    let pathspecs = selection.pathspecs(root)?;
    git(root, &["add", "-A", "--"], &pathspecs, "add")?;
    changes(root)
}

pub(crate) fn unstage(root: &Path, selection: Selection) -> Result<Changes> {
    require_repository(root)?;
    let selected = selected(root, &selection)?;
    // A staged rename is undone only together with its source.
    let paths = with_sources(selected.iter().filter(|change| change.staged.is_some()));
    if !paths.is_empty() {
        if has_head(root)? {
            git(root, &["restore", "--staged", "--"], &paths, "restore")?;
        } else {
            let args = ["rm", "--cached", "-r", "-f", "-q", "--ignore-unmatch", "--"];
            git(root, &args, &paths, "rm")?;
        }
    }
    changes(root)
}

/// Restores tracked paths from HEAD and deletes files Git reports as untracked.
pub(crate) fn discard(root: &Path, selection: Selection) -> Result<Changes> {
    require_repository(root)?;
    let selected = selected(root, &selection)?;
    let tracked = with_sources(selected.iter().filter(|change| !change.untracked()));
    if !tracked.is_empty() {
        if has_head(root)? {
            let args = ["restore", "--source=HEAD", "--staged", "--worktree", "--"];
            git(root, &args, &tracked, "restore")?;
        } else {
            git(
                root,
                &["rm", "-r", "-f", "-q", "--ignore-unmatch", "--"],
                &tracked,
                "rm",
            )?;
        }
    }
    for change in selected.iter().filter(|change| change.untracked()) {
        remove_untracked(root, &change.path)?;
    }
    changes(root)
}

pub(crate) fn commit(root: &Path, message: &str, amend: bool) -> Result<Committed> {
    let message = message.trim();
    ensure!(!message.is_empty(), "commit message must not be empty");
    require_repository(root)?;
    if !amend {
        let staged = run(root, &["diff", "--cached", "--quiet"])?;
        ensure!(!staged.success, "nothing is staged to commit");
        if !staged.different {
            staged.checked("diff")?;
        }
    }
    let mut args = vec!["commit", "-q", "-m", message];
    if amend {
        args.push("--amend");
    }
    write(root, &os(&args))?.checked("commit")?;
    Ok(Committed {
        commit: head_commit(root)?.context("commit did not create HEAD")?,
        changes: changes(root)?,
    })
}

pub(crate) fn checkout(root: &Path, branch: &str) -> Result<Changes> {
    ensure!(
        !branch.is_empty() && !branch.starts_with('-'),
        "branch must name a local branch"
    );
    require_repository(root)?;
    let reference = format!("refs/heads/{branch}");
    let exists = run(root, &["rev-parse", "--verify", "--quiet", &reference])?;
    ensure!(exists.success, "unknown local branch: {branch}");
    write(root, &os(&["switch", "-q", "--no-guess", "--", branch]))?.checked("switch")?;
    changes(root)
}

fn git(root: &Path, args: &[&str], paths: &[String], action: &str) -> Result<()> {
    let mut args = os(args);
    args.extend(paths.iter().map(OsStr::new));
    write(root, &args)?.checked(action)?;
    Ok(())
}

/// Current changes under the selection; refuses to act on a partial listing.
fn selected(root: &Path, selection: &Selection) -> Result<Vec<Change>> {
    let scoped = scan(root, &selection.pathspecs(root)?)?;
    ensure!(
        !scoped.truncated,
        "too many changes in the selection; narrow it down"
    );
    let mut entries = scoped.entries;
    if let Selection::Paths(_) = selection {
        // A pathspec hides the other side of a staged rename from status, so the
        // pair is taken from the bounded listing of the whole cwd.
        let renames = scan(root, &[".".to_owned()])?
            .entries
            .into_iter()
            .filter(|change| change.original.is_some());
        for rename in renames {
            let paired = |change: &Change| {
                change.path == rename.path || rename.original.as_ref() == Some(&change.path)
            };
            if entries.iter().any(paired) {
                entries.retain(|change| !paired(change));
                entries.push(rename);
            }
        }
    }
    Ok(entries)
}

/// Exact entry paths plus rename sources that lie inside the session cwd.
fn with_sources<'a>(changes: impl Iterator<Item = &'a Change>) -> Vec<String> {
    let mut paths = Vec::new();
    for change in changes {
        paths.push(change.path.clone());
        paths.extend(change.original.clone());
    }
    paths
}

fn remove_untracked(root: &Path, relative: &str) -> Result<()> {
    let path = Path::new(relative);
    ensure!(
        path.components()
            .all(|part| matches!(part, Component::Normal(_))),
        "invalid untracked path from Git: {relative}"
    );
    let name = path.file_name().context("untracked path has no name")?;
    // The leaf itself may be a symlink pointing anywhere: only its directory is resolved.
    let parent = match path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => validate(root, parent.to_str().context("non-UTF-8 path")?)?,
        None => root.canonicalize()?,
    };
    let target = parent.join(name);
    match fs::symlink_metadata(&target) {
        // A nested repository is reported as an untracked directory; keep it.
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Ok(fs::remove_file(target)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
