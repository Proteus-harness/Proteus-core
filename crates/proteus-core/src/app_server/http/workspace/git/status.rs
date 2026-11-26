use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::{
    super::{MAX_BYTES, MAX_ENTRIES},
    command::{Output, os, read, run},
    complete_records,
    history::{CommitSummary, head_commit},
    prefix, repository, revision, stats,
};

#[derive(Serialize)]
pub(crate) struct Changes {
    repository: bool,
    branch: Option<String>,
    head: Option<CommitSummary>,
    refs_revision: Option<String>,
    upstream: Option<String>,
    ahead: u64,
    behind: u64,
    added: u64,
    deleted: u64,
    entries: Vec<Change>,
    truncated: bool,
}

#[derive(Serialize)]
pub(super) struct Change {
    pub(super) path: String,
    pub(super) original: Option<String>,
    pub(super) status: &'static str,
    pub(super) staged: Option<&'static str>,
    pub(super) unstaged: Option<&'static str>,
    pub(super) added: Option<u64>,
    pub(super) deleted: Option<u64>,
    revision: Option<String>,
    #[serde(skip)]
    index: String,
}

impl Change {
    pub(super) fn untracked(&self) -> bool {
        self.unstaged == Some(UNTRACKED)
    }
}

const UNTRACKED: &str = "untracked";
const CONFLICT: &str = "conflict";

/// Status of the given literal pathspecs, relative to the session cwd.
pub(super) struct Scan {
    head_oid: Option<String>,
    branch: Option<String>,
    upstream: Option<String>,
    ahead: u64,
    behind: u64,
    pub(super) entries: Vec<Change>,
    pub(super) truncated: bool,
}

#[derive(Default, Serialize)]
pub(crate) struct ChangesSummary {
    repository: bool,
    branch: Option<String>,
    detached_head: Option<String>,
    upstream: Option<String>,
    ahead: u64,
    behind: u64,
    count: usize,
    truncated: bool,
}

/// The badge needs the branch, sync state and bounded number of changed paths.
/// Let Git apply the session directory pathspec; no filenames leave this call.
pub(crate) fn changes_summary(root: &Path) -> Result<ChangesSummary> {
    let output = read_status(root, &[".".to_owned()])?;
    if !output.success && !output.truncated && !repository(root)? {
        return Ok(ChangesSummary::default());
    }
    let scan = parse_status(output, "")?;
    let detached_head = if let (None, Some(oid)) = (&scan.branch, &scan.head_oid) {
        let head = run(root, &["rev-parse", "--short", oid])?.checked("rev-parse")?;
        Some(String::from_utf8(head.bytes)?.trim_end().to_owned())
    } else {
        None
    };
    Ok(ChangesSummary {
        repository: true,
        branch: scan.branch,
        detached_head,
        upstream: scan.upstream,
        ahead: scan.ahead,
        behind: scan.behind,
        count: scan.entries.len(),
        truncated: scan.truncated,
    })
}

pub(crate) fn changes(root: &Path) -> Result<Changes> {
    if !repository(root)? {
        return Ok(Changes {
            repository: false,
            branch: None,
            head: None,
            refs_revision: None,
            upstream: None,
            ahead: 0,
            behind: 0,
            added: 0,
            deleted: 0,
            entries: Vec::new(),
            truncated: false,
        });
    }
    let mut scan = scan(root, &[".".to_owned()])?;
    let refs_revision = revision::refs(root, scan.head_oid.as_deref(), scan.branch.as_deref())?;
    stats::annotate(root, &mut scan.entries)?;
    for entry in &mut scan.entries {
        entry.revision = revision::token(root, &entry.path, scan.head_oid.as_deref(), &entry.index);
    }
    Ok(Changes {
        repository: true,
        refs_revision: Some(refs_revision),
        head: if scan.head_oid.is_some() {
            head_commit(root)?
        } else {
            None
        },
        branch: scan.branch,
        upstream: scan.upstream,
        ahead: scan.ahead,
        behind: scan.behind,
        added: scan.entries.iter().filter_map(|entry| entry.added).sum(),
        deleted: scan.entries.iter().filter_map(|entry| entry.deleted).sum(),
        entries: scan.entries,
        truncated: scan.truncated,
    })
}

pub(super) fn scan(root: &Path, pathspecs: &[String]) -> Result<Scan> {
    let prefix = prefix(root)?;
    parse_status(read_status(root, pathspecs)?, &prefix)
}

fn read_status(root: &Path, pathspecs: &[String]) -> Result<Output> {
    let mut args = os(&[
        "status",
        "--porcelain=v2",
        "--branch",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=all",
        "--",
    ]);
    args.extend(pathspecs.iter().map(std::ffi::OsStr::new));
    read(root, &args, MAX_BYTES)
}

fn parse_status(output: Output, prefix: &str) -> Result<Scan> {
    ensure!(
        output.success || output.truncated,
        "cannot read Git changes"
    );
    let mut scan = Scan {
        head_oid: None,
        branch: None,
        upstream: None,
        ahead: 0,
        behind: 0,
        entries: Vec::new(),
        truncated: output.truncated,
    };
    let mut records = complete_records(&output.bytes);
    while let Some(record) = records.next() {
        let record = std::str::from_utf8(record).context("Git status is not UTF-8")?;
        if let Some(header) = record.strip_prefix("# ") {
            scan.header(header)?;
            continue;
        }
        let (path, original, staged, unstaged, index) = match record.as_bytes().first() {
            Some(b'1') => {
                let fields = status_fields::<9>(record)?;
                let (staged, unstaged) = states(fields[1])?;
                (
                    fields[8],
                    None,
                    staged,
                    unstaged,
                    revision::stage(fields[4], fields[7], 0),
                )
            }
            Some(b'2') => {
                let fields = status_fields::<10>(record)?;
                let Some(original) = records.next() else {
                    ensure!(output.truncated, "incomplete Git rename record");
                    break;
                };
                let original = std::str::from_utf8(original).context("Git path is not UTF-8")?;
                let (staged, unstaged) = states(fields[1])?;
                (
                    fields[9],
                    Some(original),
                    staged,
                    unstaged,
                    revision::stage(fields[4], fields[7], 0),
                )
            }
            Some(b'u') => {
                let fields = status_fields::<11>(record)?;
                let index = (0..3)
                    .map(|i| revision::stage(fields[3 + i], fields[7 + i], (i + 1) as u8))
                    .collect::<String>();
                (fields[10], None, Some(CONFLICT), Some(CONFLICT), index)
            }
            Some(b'?') => {
                let path = record
                    .strip_prefix("? ")
                    .context("invalid Git status record")?;
                (path, None, None, Some(UNTRACKED), String::new())
            }
            _ => continue,
        };
        let Some(path) = path.strip_prefix(prefix) else {
            continue;
        };
        if scan.entries.len() == MAX_ENTRIES {
            scan.truncated = true;
            break;
        }
        scan.entries.push(Change {
            path: path.to_owned(),
            original: original
                .and_then(|original| original.strip_prefix(prefix))
                .map(str::to_owned),
            status: combined(staged, unstaged),
            staged,
            unstaged,
            added: None,
            deleted: None,
            revision: None,
            index,
        });
    }
    Ok(scan)
}

fn status_fields<const N: usize>(record: &str) -> Result<[&str; N]> {
    let mut parts = record.splitn(N, ' ');
    let mut fields = [""; N];
    for field in &mut fields {
        let Some(part) = parts.next() else {
            anyhow::bail!("invalid Git status record");
        };
        *field = part;
    }
    Ok(fields)
}

impl Scan {
    fn header(&mut self, header: &str) -> Result<()> {
        let (key, value) = header.split_once(' ').unwrap_or((header, ""));
        match key {
            "branch.oid" => self.head_oid = (value != "(initial)").then(|| value.to_owned()),
            "branch.head" if value != "(detached)" => self.branch = Some(value.to_owned()),
            "branch.upstream" => self.upstream = Some(value.to_owned()),
            "branch.ab" => {
                let (ahead, behind) = value.split_once(' ').context("invalid Git branch.ab")?;
                self.ahead = ahead.trim_start_matches('+').parse()?;
                self.behind = behind.trim_start_matches('-').parse()?;
            }
            _ => {}
        }
        Ok(())
    }
}

fn states(xy: &str) -> Result<(Option<&'static str>, Option<&'static str>)> {
    let bytes = xy.as_bytes();
    ensure!(bytes.len() == 2, "invalid Git status code");
    Ok((state(bytes[0]), state(bytes[1])))
}

fn state(code: u8) -> Option<&'static str> {
    match code {
        b'.' => None,
        b'A' | b'C' => Some("added"),
        b'D' => Some("deleted"),
        b'R' => Some("renamed"),
        b'U' => Some(CONFLICT),
        _ => Some("modified"),
    }
}

fn combined(staged: Option<&'static str>, unstaged: Option<&'static str>) -> &'static str {
    let sides = [staged, unstaged];
    ["conflict", UNTRACKED, "renamed", "deleted", "added"]
        .into_iter()
        .find(|status| sides.contains(&Some(status)))
        .unwrap_or("modified")
}
