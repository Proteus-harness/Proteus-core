//! Paged commit history, optionally across all branches for a commit graph.
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::{command::run, repository};

pub(crate) const DEFAULT_LOG_ENTRIES: usize = 50;
const MAX_LOG_ENTRIES: usize = 500;
const FIELDS: usize = 8;

pub(crate) struct LogQuery {
    pub(crate) limit: usize,
    pub(crate) skip: usize,
    pub(crate) all: bool,
}

#[derive(Serialize)]
pub(crate) struct Log {
    head: Option<String>,
    entries: Vec<LogEntry>,
    has_more: bool,
}

#[derive(Serialize)]
struct LogEntry {
    hash: String,
    short: String,
    parents: Vec<String>,
    summary: String,
    author: String,
    email: String,
    time: i64,
    refs: Vec<Ref>,
}

#[derive(Serialize)]
struct Ref {
    name: String,
    kind: &'static str,
}

pub(super) fn head_hash(root: &Path) -> Result<Option<String>> {
    let output = run(root, &["rev-parse", "--verify", "--quiet", "HEAD"])?;
    Ok(output
        .success
        .then(|| String::from_utf8_lossy(&output.bytes).trim().to_owned()))
}

/// Commits of HEAD (or every branch, remote branch and tag with `all`)
/// newest first; one extra commit is read to report `has_more`.
pub(crate) fn log(root: &Path, query: LogQuery) -> Result<Log> {
    ensure!(
        (1..=MAX_LOG_ENTRIES).contains(&query.limit),
        "log limit must be between 1 and {MAX_LOG_ENTRIES}"
    );
    let mut result = Log {
        head: None,
        entries: Vec::new(),
        has_more: false,
    };
    if !repository(root)? {
        return Ok(result);
    }
    result.head = head_hash(root)?;
    if result.head.is_none() {
        return Ok(result);
    }
    let count = format!("--max-count={}", query.limit + 1);
    let skip = format!("--skip={}", query.skip);
    let mut args = vec![
        "log",
        "-z",
        &count,
        &skip,
        "--no-show-signature",
        "--decorate=full",
        "--format=%H%x00%h%x00%P%x00%an%x00%ae%x00%ct%x00%D%x00%s",
    ];
    if query.all {
        args.extend(["--date-order", "--branches", "--remotes", "--tags"]);
    }
    args.extend(["HEAD", "--"]);
    let output = run(root, &args)?.checked_or_truncated("log")?;
    // Stopped at the output cap: whatever was not read is still to come.
    result.has_more = output.truncated;
    let text = String::from_utf8_lossy(&output.bytes);
    let fields: Vec<&str> = text.split('\0').collect();
    // A trailing record cut at the cap, or the final empty terminator, is incomplete.
    let (records, _) = fields.as_chunks::<FIELDS>();
    for [hash, short, parents, author, email, time, refs, summary] in records {
        if result.entries.len() == query.limit {
            result.has_more = true;
            break;
        }
        result.entries.push(LogEntry {
            hash: (*hash).to_owned(),
            short: (*short).to_owned(),
            parents: parents.split_whitespace().map(str::to_owned).collect(),
            summary: (*summary).to_owned(),
            author: (*author).to_owned(),
            email: (*email).to_owned(),
            time: time.parse().context("invalid Git commit time")?,
            refs: decorations(refs),
        });
    }
    Ok(result)
}

/// `%D` with `--decorate=full`, e.g. `HEAD -> refs/heads/main, tag: refs/tags/v1`.
fn decorations(text: &str) -> Vec<Ref> {
    let mut refs = Vec::new();
    for item in text.split(", ").filter(|item| !item.is_empty()) {
        let item = match item.strip_prefix("HEAD -> ") {
            Some(target) => {
                refs.push(head());
                target
            }
            None if item == "HEAD" => {
                refs.push(head());
                continue;
            }
            None => item,
        };
        let classified = [
            ("tag: refs/tags/", "tag"),
            ("refs/heads/", "branch"),
            ("refs/remotes/", "remote"),
        ]
        .into_iter()
        .find_map(|(prefix, kind)| item.strip_prefix(prefix).map(|name| (name, kind)));
        if let Some((name, kind)) = classified {
            refs.push(Ref {
                name: name.to_owned(),
                kind,
            });
        }
    }
    refs
}

fn head() -> Ref {
    Ref {
        name: "HEAD".to_owned(),
        kind: "head",
    }
}
