use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::{super::MAX_ENTRIES, command::run, has_head, repository};

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
    last_commit_summary: String,
    author: String,
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
            "--format=%(refname:lstrip=2)%00%(upstream:short)%00%(committerdate:unix)%00%(authorname)%00%(contents:subject)",
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
        let (Some(name), Some(upstream), Some(time), Some(author), Some(summary), None) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            anyhow::bail!("invalid Git branch record");
        };
        result.branches.push(Branch {
            name: name.to_owned(),
            upstream: (!upstream.is_empty()).then(|| upstream.to_owned()),
            last_commit_time: time.parse().ok(),
            last_commit_summary: summary.to_owned(),
            author: author.to_owned(),
        });
    }
    Ok(result)
}
