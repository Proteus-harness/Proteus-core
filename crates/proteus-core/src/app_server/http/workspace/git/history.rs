use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::{super::MAX_ENTRIES, command::run, has_head, repository};

pub(crate) const MAX_LOG_ENTRIES: usize = 100;

#[derive(Serialize)]
pub(crate) struct CommitSummary {
    short: String,
    summary: String,
}

#[derive(Serialize)]
pub(crate) struct Branches {
    current: Option<String>,
    branches: Vec<Branch>,
    truncated: bool,
}

#[derive(Serialize)]
struct Branch {
    name: String,
    upstream: Option<String>,
    last_commit_time: Option<i64>,
}

#[derive(Serialize)]
pub(crate) struct Log {
    entries: Vec<LogEntry>,
}

#[derive(Serialize)]
struct LogEntry {
    hash: String,
    short: String,
    summary: String,
    author: String,
    time: i64,
}

pub(super) fn head_commit(root: &Path) -> Result<Option<CommitSummary>> {
    if !has_head(root)? {
        return Ok(None);
    }
    let output = run(
        root,
        &[
            "log",
            "-1",
            "--no-show-signature",
            "--format=%h%x00%s",
            "HEAD",
            "--",
        ],
    )?
    .checked("log")?;
    let text = String::from_utf8_lossy(&output.bytes);
    let (short, summary) = text
        .trim_end_matches('\n')
        .split_once('\0')
        .context("invalid Git commit summary")?;
    Ok(Some(CommitSummary {
        short: short.to_owned(),
        summary: summary.to_owned(),
    }))
}

/// Local branches, most recently committed first.
pub(crate) fn branches(root: &Path) -> Result<Branches> {
    let mut result = Branches {
        current: None,
        branches: Vec::new(),
        truncated: false,
    };
    if !repository(root)? {
        return Ok(result);
    }
    let head = run(root, &["symbolic-ref", "--quiet", "HEAD"])?;
    if head.success {
        result.current = String::from_utf8_lossy(&head.bytes)
            .trim_end()
            .strip_prefix("refs/heads/")
            .map(str::to_owned);
    }
    let count = format!("--count={}", MAX_ENTRIES + 1);
    let output = run(
        root,
        &[
            "for-each-ref",
            &count,
            "--sort=refname",
            "--sort=-committerdate",
            "--format=%(refname:lstrip=2)%00%(upstream:short)%00%(committerdate:unix)",
            "refs/heads",
        ],
    )?;
    ensure!(
        output.success || output.truncated,
        "cannot list Git branches"
    );
    result.truncated = output.truncated;
    let text = String::from_utf8_lossy(&output.bytes);
    // Ref names cannot contain newlines; a line cut at the cap is incomplete.
    let complete = text.rfind('\n').map_or("", |end| &text[..end]);
    for line in complete.split('\n').filter(|line| !line.is_empty()) {
        if result.branches.len() == MAX_ENTRIES {
            result.truncated = true;
            break;
        }
        let mut fields = line.split('\0');
        let (Some(name), Some(upstream), Some(time), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            anyhow::bail!("invalid Git branch record");
        };
        result.branches.push(Branch {
            name: name.to_owned(),
            upstream: (!upstream.is_empty()).then(|| upstream.to_owned()),
            last_commit_time: time.parse().ok(),
        });
    }
    Ok(result)
}

/// The newest `limit` commits reachable from HEAD of the whole repository.
pub(crate) fn log(root: &Path, limit: usize) -> Result<Log> {
    ensure!(
        (1..=MAX_LOG_ENTRIES).contains(&limit),
        "log limit must be between 1 and {MAX_LOG_ENTRIES}"
    );
    let mut result = Log {
        entries: Vec::new(),
    };
    if !repository(root)? || !has_head(root)? {
        return Ok(result);
    }
    let count = format!("--max-count={limit}");
    let output = run(
        root,
        &[
            "log",
            "-z",
            &count,
            "--no-show-signature",
            "--format=%H%x00%h%x00%an%x00%ct%x00%s",
            "HEAD",
            "--",
        ],
    )?
    .checked("log")?;
    ensure!(!output.truncated, "Git log output is too large");
    let text = String::from_utf8_lossy(&output.bytes);
    let fields: Vec<&str> = text
        .strip_suffix('\0')
        .unwrap_or(&text)
        .split('\0')
        .collect();
    let (records, remainder) = fields.as_chunks::<5>();
    ensure!(remainder.is_empty(), "invalid Git log record");
    for [hash, short, author, time, summary] in records {
        result.entries.push(LogEntry {
            hash: (*hash).to_owned(),
            short: (*short).to_owned(),
            author: (*author).to_owned(),
            time: time.parse().context("invalid Git commit time")?,
            summary: (*summary).to_owned(),
        });
    }
    Ok(result)
}
