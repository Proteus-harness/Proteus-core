//! Line counts of changes: one bounded `git diff --numstat` plus capped
//! reads of untracked files.
use std::{collections::HashMap, fs, io::Read, path::Path};

use anyhow::Result;

use super::{
    super::MAX_BYTES,
    command::{os, read},
    complete_records, empty_tree, has_head,
    status::Change,
};

/// Untracked files above this size, or past the shared budget, are not counted.
const MAX_UNTRACKED_FILE_BYTES: u64 = 512 * 1024;
const MAX_UNTRACKED_TOTAL_BYTES: u64 = 4 * 1024 * 1024;

/// `None` marks a binary file or one that was not counted.
pub(super) type LineCounts = Option<(u64, u64)>;

/// Fills `added`/`deleted` of entries from the work tree against HEAD.
pub(super) fn annotate(root: &Path, entries: &mut [Change]) -> Result<()> {
    let base = if has_head(root)? {
        "HEAD".to_owned()
    } else {
        empty_tree(root)?
    };
    let output = read(
        root,
        &os(&[
            "diff",
            "--numstat",
            "-z",
            "-M",
            "--relative",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=all",
            &base,
            "--",
        ]),
        MAX_BYTES,
    )?;
    let counts = if output.success || output.truncated {
        parse_numstat(&output.bytes)
    } else {
        HashMap::new()
    };
    let mut budget = MAX_UNTRACKED_TOTAL_BYTES;
    for entry in entries {
        let lines = if entry.untracked() {
            count_untracked(root, &entry.path, &mut budget)
        } else {
            counts.get(&entry.path).copied().flatten()
        };
        entry.added = lines.map(|(added, _)| added);
        entry.deleted = lines.map(|(_, deleted)| deleted);
    }
    Ok(())
}

/// `-z` numstat keyed by the new path; a record cut at the output cap is dropped.
pub(super) fn parse_numstat(bytes: &[u8]) -> HashMap<String, LineCounts> {
    let mut counts = HashMap::new();
    let mut records = complete_records(bytes);
    while let Some(record) = records.next() {
        let Ok(record) = std::str::from_utf8(record) else {
            continue;
        };
        let mut fields = record.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            // A rename: the source and destination follow as separate records.
            let (Some(_), Some(path)) = (records.next(), records.next()) else {
                break;
            };
            String::from_utf8_lossy(path).into_owned()
        } else {
            path.to_owned()
        };
        let lines = added.parse().ok().zip(deleted.parse().ok());
        counts.insert(path, lines);
    }
    counts
}

fn count_untracked(root: &Path, relative: &str, budget: &mut u64) -> LineCounts {
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path).ok()?;
    let size = metadata.len();
    if !metadata.is_file() || size > MAX_UNTRACKED_FILE_BYTES || size > *budget {
        return None;
    }
    *budget -= size;
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_UNTRACKED_FILE_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let newlines = bytes.iter().filter(|byte| **byte == b'\n').count() as u64;
    let partial = u64::from(bytes.last().is_some_and(|byte| *byte != b'\n'));
    Some((newlines + partial, 0))
}
