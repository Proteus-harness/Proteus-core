use std::collections::{BTreeSet, HashMap, HashSet};

use super::{ChangeStatus, Changes, EntryKind, Listing};

/// Lazily listed workspace tree. Directories are listed on first expansion;
/// git status marks files and the directories that contain changes.
#[derive(Default)]
pub struct FileTree {
    listings: HashMap<String, Listing>,
    expanded: BTreeSet<String>,
    loading: HashSet<String>,
    statuses: HashMap<String, ChangeStatus>,
    changed_dirs: HashSet<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TreeRow {
    pub path: String,
    pub name: String,
    pub kind: EntryKind,
    pub depth: usize,
    pub expanded: bool,
    pub loading: bool,
    pub status: Option<ChangeStatus>,
    pub contains_changes: bool,
    /// The directory listing stopped at the server limit.
    pub truncated: bool,
}

impl FileTree {
    /// Marks a directory as loading; `false` when it is already listed or
    /// loading, so the caller does not request it twice.
    pub fn begin_loading(&mut self, dir: &str) -> bool {
        if self.listings.contains_key(dir) {
            return false;
        }
        self.loading.insert(dir.to_string())
    }

    pub fn finish_loading(&mut self, dir: &str) {
        self.loading.remove(dir);
    }

    pub fn insert(&mut self, listing: Listing) {
        self.loading.remove(&listing.path);
        self.listings.insert(listing.path.clone(), listing);
    }

    /// Directories listed so far, to re-read them after the workspace changed.
    pub fn listed(&self) -> Vec<String> {
        self.listings.keys().cloned().collect()
    }

    /// Forgets a directory that disappeared together with its subdirectories.
    pub fn remove(&mut self, dir: &str) {
        let prefix = format!("{dir}/");
        self.listings
            .retain(|path, _| path != dir && !path.starts_with(&prefix));
        self.expanded
            .retain(|path| path != dir && !path.starts_with(&prefix));
    }

    /// The root and the visible expanded directories that still need a listing.
    pub fn missing_listings(&self, rows: &[TreeRow]) -> Vec<String> {
        let visible = rows
            .iter()
            .filter(|row| row.expanded)
            .map(|row| row.path.clone());
        std::iter::once(String::new())
            .chain(visible)
            .filter(|dir| !self.listings.contains_key(dir) && !self.loading.contains(dir))
            .collect()
    }

    pub fn toggle(&mut self, dir: &str) -> bool {
        if !self.expanded.remove(dir) {
            self.expanded.insert(dir.to_string());
            return true;
        }
        false
    }

    /// Expands every ancestor of `path` so it becomes visible.
    pub fn reveal(&mut self, path: &str) {
        for (index, _) in path.match_indices('/') {
            self.expanded.insert(path[..index].to_string());
        }
    }

    pub fn set_changes(&mut self, changes: &Changes) {
        self.statuses = changes
            .entries
            .iter()
            .map(|change| (change.path.clone(), change.status))
            .collect();
        self.changed_dirs = changes
            .entries
            .iter()
            .flat_map(|change| {
                change
                    .path
                    .match_indices('/')
                    .map(|(index, _)| change.path[..index].to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
    }

    /// Visible rows in display order. A non-empty filter keeps matching
    /// entries of the listed directories together with their ancestors and
    /// shows them expanded.
    pub fn rows(&self, filter: &str) -> Vec<TreeRow> {
        let filter = filter.trim().to_lowercase();
        let mut rows = Vec::new();
        self.push_rows("", 0, &filter, &mut rows);
        rows
    }

    fn push_rows(&self, dir: &str, depth: usize, filter: &str, rows: &mut Vec<TreeRow>) {
        let Some(listing) = self.listings.get(dir) else {
            return;
        };
        for entry in &listing.entries {
            let directory = entry.kind == EntryKind::Directory;
            let own_match = filter.is_empty() || entry.name.to_lowercase().contains(filter);
            if !own_match && !self.matches(&entry.path, &entry.name, directory, filter) {
                continue;
            }
            // A directory matching by its own name shows its children as is;
            // otherwise it opens to show the matches inside.
            let child_filter = if own_match { "" } else { filter };
            let expanded = directory && (self.expanded.contains(&entry.path) || !own_match);
            rows.push(TreeRow {
                path: entry.path.clone(),
                name: entry.name.clone(),
                kind: entry.kind,
                depth,
                expanded,
                loading: self.loading.contains(&entry.path),
                status: self.statuses.get(&entry.path).copied(),
                contains_changes: directory && self.changed_dirs.contains(&entry.path),
                truncated: false,
            });
            if expanded {
                self.push_rows(&entry.path, depth + 1, child_filter, rows);
            }
        }
        if listing.truncated
            && let Some(last) = rows.last_mut()
        {
            last.truncated = true;
        }
    }

    fn matches(&self, path: &str, name: &str, directory: bool, filter: &str) -> bool {
        if name.to_lowercase().contains(filter) {
            return true;
        }
        directory
            && self.listings.get(path).is_some_and(|listing| {
                listing.entries.iter().any(|entry| {
                    self.matches(
                        &entry.path,
                        &entry.name,
                        entry.kind == EntryKind::Directory,
                        filter,
                    )
                })
            })
    }
}
