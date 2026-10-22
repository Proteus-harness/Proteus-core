use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::{
    super::{MAX_BYTES, MAX_ENTRIES},
    command::{os, read},
    complete_records,
    history::{CommitSummary, head_commit},
    prefix, repository, stats,
};

#[derive(Serialize)]
pub(crate) struct Changes {
    repository: bool,
    branch: Option<String>,
    head: Option<CommitSummary>,
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
    head_oid: bool,
    branch: Option<String>,
    upstream: Option<String>,
    ahead: u64,
    behind: u64,
    pub(super) entries: Vec<Change>,
    pub(super) truncated: bool,
}

pub(crate) fn changes(root: &Path) -> Result<Changes> {
    if !repository(root)? {
        return Ok(Changes {
            repository: false,
            branch: None,
            head: None,
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
    stats::annotate(root, &mut scan.entries)?;
    Ok(Changes {
        repository: true,
        head: if scan.head_oid {
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
    let output = read(root, &args, MAX_BYTES)?;
    ensure!(
        output.success || output.truncated,
        "cannot read Git changes"
    );
    let mut scan = Scan {
        head_oid: false,
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
        let (path, original, staged, unstaged) = match record.as_bytes().first() {
            Some(b'1') => {
                let fields: Vec<_> = record.splitn(9, ' ').collect();
                ensure!(fields.len() == 9, "invalid Git status record");
                let (staged, unstaged) = states(fields[1])?;
                (fields[8], None, staged, unstaged)
            }
            Some(b'2') => {
                let fields: Vec<_> = record.splitn(10, ' ').collect();
                ensure!(fields.len() == 10, "invalid Git status record");
                let Some(original) = records.next() else {
                    ensure!(output.truncated, "incomplete Git rename record");
                    break;
                };
                let original = std::str::from_utf8(original).context("Git path is not UTF-8")?;
                let (staged, unstaged) = states(fields[1])?;
                (fields[9], Some(original), staged, unstaged)
            }
            Some(b'u') => {
                let fields: Vec<_> = record.splitn(11, ' ').collect();
                ensure!(fields.len() == 11, "invalid Git status record");
                (fields[10], None, Some(CONFLICT), Some(CONFLICT))
            }
            Some(b'?') => {
                let path = record
                    .strip_prefix("? ")
                    .context("invalid Git status record")?;
                (path, None, None, Some(UNTRACKED))
            }
            _ => continue,
        };
        let Some(path) = path.strip_prefix(prefix.as_str()) else {
            continue;
        };
        if scan.entries.len() == MAX_ENTRIES {
            scan.truncated = true;
            break;
        }
        scan.entries.push(Change {
            path: path.to_owned(),
            original: original
                .and_then(|original| original.strip_prefix(prefix.as_str()))
                .map(str::to_owned),
            status: combined(staged, unstaged),
            staged,
            unstaged,
            added: None,
            deleted: None,
        });
    }
    Ok(scan)
}

impl Scan {
    fn header(&mut self, header: &str) -> Result<()> {
        let (key, value) = header.split_once(' ').unwrap_or((header, ""));
        match key {
            "branch.oid" => self.head_oid = value != "(initial)",
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
