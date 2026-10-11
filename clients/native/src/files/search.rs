use std::collections::{BTreeMap, BTreeSet};

use super::{EntryKind, FileTree, SearchResult, TreeRow};

/// Rows for a whole-workspace search: matches in tree order under their
/// ancestors, which are shown expanded. A matched directory stays
/// collapsed; its contents belong to the regular tree.
pub fn search_rows(tree: &FileTree, result: &SearchResult) -> Vec<TreeRow> {
    let mut kinds: BTreeMap<&str, EntryKind> = BTreeMap::new();
    let mut children: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for entry in &result.entries {
        kinds.insert(&entry.path, entry.kind);
        let mut child = entry.path.as_str();
        for (index, _) in entry.path.match_indices('/').rev() {
            let parent = &entry.path[..index];
            children.entry(parent).or_default().insert(child);
            kinds.entry(parent).or_insert(EntryKind::Directory);
            child = parent;
        }
        children.entry("").or_default().insert(child);
    }
    let mut rows = Vec::new();
    push(tree, "", 0, &kinds, &children, &mut rows);
    rows
}

fn push(
    tree: &FileTree,
    dir: &str,
    depth: usize,
    kinds: &BTreeMap<&str, EntryKind>,
    children: &BTreeMap<&str, BTreeSet<&str>>,
    rows: &mut Vec<TreeRow>,
) {
    let Some(paths) = children.get(dir) else {
        return;
    };
    let mut paths: Vec<(&str, EntryKind)> = paths.iter().map(|path| (*path, kinds[path])).collect();
    paths.sort_by_cached_key(|(path, kind)| {
        (*kind != EntryKind::Directory, name(path).to_lowercase())
    });
    for (path, kind) in paths {
        let directory = kind == EntryKind::Directory;
        let expanded = children.contains_key(path);
        let (status, contains_changes) = tree.marks(path, directory);
        rows.push(TreeRow {
            path: path.to_string(),
            name: name(path).to_string(),
            kind,
            depth,
            expanded,
            loading: false,
            status,
            contains_changes,
            truncated: false,
        });
        if expanded {
            push(tree, path, depth + 1, kinds, children, rows);
        }
    }
}

fn name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}
