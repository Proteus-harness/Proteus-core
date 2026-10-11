//! Fuzzy search over workspace file and directory paths. Inside a Git
//! repository the candidates are the files Git does not ignore; elsewhere
//! a bounded walk that skips `.git` provides them.
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail, ensure};
use serde::Serialize;

use super::WorkspaceEntry;

const MAX_RESULTS: usize = 200;
const MAX_WALK_ENTRIES: usize = 50_000;
const MAX_LISTING_BYTES: usize = 32 * 1024 * 1024;

#[derive(Serialize)]
pub(crate) struct WorkspaceSearch {
    query: String,
    entries: Vec<WorkspaceEntry>,
    truncated: bool,
}

pub(crate) fn query_text(query: Option<&str>) -> Result<String> {
    let mut text = None;
    for pair in query
        .unwrap_or_default()
        .split('&')
        .filter(|s| !s.is_empty())
    {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "q" if text.is_none() => {
                text = Some(super::super::sessions::percent_decode_query_value(value)?)
            }
            "session_dir" | "token" => {}
            _ => bail!("unknown or duplicate workspace search parameter: {key}"),
        }
    }
    let text = text.unwrap_or_default();
    ensure!(!text.trim().is_empty(), "search requires a non-empty q");
    Ok(text)
}

pub(crate) fn search(root: &Path, query: String) -> Result<WorkspaceSearch> {
    let terms: Vec<Vec<char>> = query
        .split_whitespace()
        .map(|term| term.to_lowercase().chars().collect())
        .collect();
    let (candidates, mut truncated) = candidates(root)?;
    let mut scored: Vec<(i64, &Candidate)> = candidates
        .iter()
        .filter_map(|candidate| Some((score(&terms, &candidate.path)?, candidate)))
        .collect();
    scored.sort_by(|(a, x), (b, y)| {
        b.cmp(a)
            .then(x.path.len().cmp(&y.path.len()))
            .then(x.path.cmp(&y.path))
    });
    let mut entries = Vec::new();
    for (_, candidate) in scored {
        if entries.len() == MAX_RESULTS {
            truncated = true;
            break;
        }
        // Tracked files deleted from the work tree are still in the index.
        if fs::symlink_metadata(root.join(&candidate.path)).is_err() {
            continue;
        }
        let name = candidate
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&candidate.path)
            .to_owned();
        entries.push(WorkspaceEntry {
            name,
            path: candidate.path.clone(),
            kind: if candidate.directory {
                "directory"
            } else {
                "file"
            },
        });
    }
    Ok(WorkspaceSearch {
        query,
        entries,
        truncated,
    })
}

struct Candidate {
    path: String,
    directory: bool,
}

fn candidates(root: &Path) -> Result<(Vec<Candidate>, bool)> {
    let root = root.canonicalize()?;
    let (files, truncated) = match super::git::visible_files(&root, MAX_LISTING_BYTES)? {
        Some(listing) => listing,
        None => walk(&root),
    };
    // Directories are not listed by Git; they come from the file paths.
    let directories: BTreeSet<&str> = files
        .iter()
        .flat_map(|path| path.match_indices('/').map(|(index, _)| &path[..index]))
        .collect();
    let mut candidates: Vec<Candidate> = directories
        .into_iter()
        .map(|path| Candidate {
            path: path.to_owned(),
            directory: true,
        })
        .collect();
    candidates.extend(files.into_iter().map(|path| Candidate {
        path,
        directory: false,
    }));
    Ok((candidates, truncated))
}

/// Non-directory entries below `root`, without following symlinks.
fn walk(root: &Path) -> (Vec<String>, bool) {
    let mut files = Vec::new();
    let mut pending: Vec<PathBuf> = vec![PathBuf::new()];
    let mut seen = 0;
    while let Some(dir) = pending.pop() {
        let Ok(read) = fs::read_dir(root.join(&dir)) else {
            continue;
        };
        for entry in read.flatten() {
            seen += 1;
            if seen > MAX_WALK_ENTRIES {
                return (files, true);
            }
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let relative = dir.join(&name);
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if name != ".git" {
                    pending.push(relative);
                }
            } else if let Some(path) = relative.to_str() {
                files.push(path.to_owned());
            }
        }
    }
    (files, false)
}

/// Every term must match the path; `None` otherwise. Higher is better:
/// a match in the file name beats one spread over directories, a
/// contiguous match beats a scattered subsequence.
fn score(terms: &[Vec<char>], path: &str) -> Option<i64> {
    let lower = path.to_lowercase();
    let name_start = lower.rfind('/').map_or(0, |index| index + 1);
    let name = &lower[name_start..];
    let mut total = 0;
    for term in terms {
        let text: String = term.iter().collect();
        total += if name == text {
            4000
        } else if name.starts_with(&text) {
            3000
        } else if name.contains(&text) {
            2000
        } else if lower.contains(&text) {
            1000
        } else {
            subsequence(term, &lower, name_start)?
        };
    }
    Some(total - path.matches('/').count() as i64)
}

/// Scattered match: points for characters that start a segment or fall in
/// the file name, penalties for gaps.
fn subsequence(term: &[char], path: &str, name_start: usize) -> Option<i64> {
    let mut score = 0;
    let mut chars = path.char_indices().peekable();
    let mut previous: Option<char> = None;
    let mut next: Option<usize> = None;
    for &wanted in term {
        loop {
            let (index, current) = chars.next()?;
            let before = previous;
            previous = Some(current);
            if current != wanted {
                continue;
            }
            if matches!(before, None | Some('/' | '_' | '-' | '.' | ' ')) {
                score += 30;
            }
            if index >= name_start {
                score += 10;
            }
            match next {
                Some(expected) if index == expected => score += 15,
                Some(_) => score -= 3,
                None => {}
            }
            next = Some(index + current.len_utf8());
            break;
        }
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(query: &str) -> Vec<Vec<char>> {
        query
            .split_whitespace()
            .map(|term| term.to_lowercase().chars().collect())
            .collect()
    }

    #[test]
    fn name_matches_rank_above_scattered_ones() {
        let query = terms("tree");
        let exact = score(&query, "src/files/tree").unwrap();
        let prefix = score(&query, "src/files/tree.rs").unwrap();
        let inside = score(&query, "src/subtree.rs").unwrap();
        let directory = score(&query, "tree/mod.rs").unwrap();
        let scattered = score(&query, "src/t_r_e_e.rs").unwrap();
        assert!(exact > prefix && prefix > inside && inside > directory);
        assert!(directory > scattered);
        assert_eq!(score(&query, "src/main.rs"), None);
    }

    #[test]
    fn every_term_must_match() {
        assert!(score(&terms("files tree"), "src/files/tree.rs").is_some());
        assert_eq!(score(&terms("files graph"), "src/files/tree.rs"), None);
        assert!(score(&terms("FTR"), "src/files/tree.rs").is_some());
    }
}
