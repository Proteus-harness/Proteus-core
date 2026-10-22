//! One commit of the history: metadata, changed files and its patch.
use std::path::{Component, Path};

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use super::{
    super::{MAX_BYTES, MAX_ENTRIES},
    command::{os, read, run},
    complete_records,
    diff::Diff,
    empty_tree, prefix, require_repository,
    stats::parse_numstat,
    toplevel,
};

#[derive(Serialize)]
pub(crate) struct CommitDetails {
    hash: String,
    short: String,
    summary: String,
    body: String,
    author: String,
    email: String,
    time: i64,
    parents: Vec<String>,
    prefix: String,
    files: Vec<CommitFile>,
    files_truncated: bool,
    patch: String,
    truncated: bool,
}

#[derive(Serialize)]
struct CommitFile {
    path: String,
    original: Option<String>,
    status: &'static str,
    added: Option<u64>,
    deleted: Option<u64>,
}

const DIFF_FLAGS: [&str; 5] = ["diff", "-M", "--no-color", "--no-ext-diff", "--no-textconv"];

/// Metadata and diff of a commit against its first parent (a root commit
/// against the empty tree). Paths are relative to the repository root.
pub(crate) fn commit_details(root: &Path, hash: &str) -> Result<CommitDetails> {
    let (hash, parents) = resolve(root, hash)?;
    let output = run(
        root,
        &[
            "log",
            "-1",
            "--no-show-signature",
            "--format=%h%x00%an%x00%ae%x00%ct%x00%s%x00%b",
            &hash,
            "--",
        ],
    )?
    .checked("log")?;
    let text = String::from_utf8_lossy(&output.bytes);
    let fields: Vec<&str> = text.splitn(6, '\0').collect();
    let [short, author, email, time, summary, body] = fields[..] else {
        bail!("invalid Git commit record");
    };
    let base = base(root, &parents)?;
    let top = toplevel(root)?;
    let (files, files_truncated) = files(&top, &base, &hash)?;
    let mut args = DIFF_FLAGS.to_vec();
    args.extend([base.as_str(), hash.as_str(), "--"]);
    let patch = read(&top, &os(&args), MAX_BYTES)?.checked_or_truncated("diff")?;
    let mut bytes = patch.bytes;
    if patch.truncated {
        // Cut at a line boundary so clients never see half a line.
        bytes.truncate(bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1));
    }
    Ok(CommitDetails {
        short: short.to_owned(),
        summary: summary.to_owned(),
        body: body.trim_end().to_owned(),
        author: author.to_owned(),
        email: email.to_owned(),
        time: time.parse().context("invalid Git commit time")?,
        prefix: prefix(root)?,
        hash,
        parents,
        files,
        files_truncated,
        patch: String::from_utf8_lossy(&bytes).into_owned(),
        truncated: patch.truncated,
    })
}

/// The diff of one repository-relative file in a commit, shaped like `/workspace/diff`.
pub(crate) fn commit_diff(root: &Path, hash: &str, path: String) -> Result<Diff> {
    ensure!(!path.is_empty(), "a repository path is required");
    ensure!(
        Path::new(&path)
            .components()
            .all(|part| matches!(part, Component::Normal(_))),
        "commit path must be relative to the repository root and cannot contain '..'"
    );
    let (hash, parents) = resolve(root, hash)?;
    let base = base(root, &parents)?;
    let mut args = DIFF_FLAGS.to_vec();
    args.extend([base.as_str(), hash.as_str(), "--", path.as_str()]);
    let output = read(&toplevel(root)?, &os(&args), MAX_BYTES)?;
    if output.success && !output.truncated && output.bytes.is_empty() {
        return Ok(Diff::unavailable(path));
    }
    Diff::unavailable(path).classify(output)
}

/// Full hash and parents of a hex commit id (4..64 digits).
fn resolve(root: &Path, hash: &str) -> Result<(String, Vec<String>)> {
    ensure!(
        (4..=64).contains(&hash.len()) && hash.bytes().all(|b| b.is_ascii_hexdigit()),
        "commit hash must be 4 to 64 hexadecimal digits"
    );
    require_repository(root)?;
    let revision = format!("{hash}^{{commit}}");
    let output = run(root, &["rev-parse", "--verify", "--quiet", &revision])?;
    ensure!(output.success, "unknown commit: {hash}");
    let full = String::from_utf8_lossy(&output.bytes).trim().to_owned();
    let output = run(root, &["log", "-1", "--format=%P", &full, "--"])?.checked("log")?;
    let parents = String::from_utf8_lossy(&output.bytes)
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    Ok((full, parents))
}

fn base(root: &Path, parents: &[String]) -> Result<String> {
    match parents.first() {
        Some(parent) => Ok(parent.clone()),
        None => empty_tree(root),
    }
}

fn files(top: &Path, base: &str, hash: &str) -> Result<(Vec<CommitFile>, bool)> {
    let mut args = DIFF_FLAGS.to_vec();
    args.extend(["--numstat", "-z", base, hash, "--"]);
    let numstat = read(top, &os(&args), MAX_BYTES)?;
    let counts = parse_numstat(&numstat.bytes);
    args.truncate(DIFF_FLAGS.len());
    args.extend(["--name-status", "-z", base, hash, "--"]);
    let output = read(top, &os(&args), MAX_BYTES)?.checked_or_truncated("diff")?;
    let mut truncated = output.truncated;
    let mut files = Vec::new();
    let mut records = complete_records(&output.bytes).map(String::from_utf8_lossy);
    while let Some(code) = records.next() {
        let renamed = code.starts_with(['R', 'C']);
        let original = if renamed { records.next() } else { None };
        let Some(path) = records.next() else { break };
        if files.len() == MAX_ENTRIES {
            truncated = true;
            break;
        }
        let lines = counts.get(path.as_ref()).copied().flatten();
        files.push(CommitFile {
            status: match code.as_bytes().first() {
                Some(b'A') | Some(b'C') => "added",
                Some(b'D') => "deleted",
                Some(b'R') => "renamed",
                _ => "modified",
            },
            original: original.map(|original| original.into_owned()),
            path: path.into_owned(),
            added: lines.map(|(added, _)| added),
            deleted: lines.map(|(_, deleted)| deleted),
        });
    }
    Ok((files, truncated))
}
