use super::{
    Change, ChangeStatus, Changes, Entry, EntryKind, FileTree, Listing, language_for, split_path,
};

fn listing(path: &str, entries: &[(&str, EntryKind)]) -> Listing {
    Listing {
        path: path.to_string(),
        entries: entries
            .iter()
            .map(|(name, kind)| Entry {
                name: name.to_string(),
                path: if path.is_empty() {
                    name.to_string()
                } else {
                    format!("{path}/{name}")
                },
                kind: *kind,
            })
            .collect(),
        truncated: false,
    }
}

fn paths(tree: &FileTree, filter: &str) -> Vec<String> {
    tree.rows(filter).into_iter().map(|row| row.path).collect()
}

#[test]
fn tree_lists_lazily_and_marks_changes() {
    let mut tree = FileTree::default();
    assert_eq!(tree.missing_listings(&tree.rows("")), vec![String::new()]);
    tree.insert(listing(
        "",
        &[
            ("src", EntryKind::Directory),
            ("README.md", EntryKind::File),
        ],
    ));
    assert_eq!(paths(&tree, ""), ["src", "README.md"]);

    assert!(tree.toggle("src"));
    let rows = tree.rows("");
    assert!(rows[0].expanded);
    assert_eq!(tree.missing_listings(&rows), ["src"]);
    tree.insert(listing(
        "src",
        &[("ui", EntryKind::Directory), ("main.rs", EntryKind::File)],
    ));
    assert_eq!(
        paths(&tree, ""),
        ["src", "src/ui", "src/main.rs", "README.md"]
    );

    tree.set_changes(&Changes {
        repository: true,
        entries: vec![Change {
            path: "src/main.rs".to_string(),
            status: ChangeStatus::Modified,
        }],
        truncated: false,
    });
    let rows = tree.rows("");
    assert!(rows[0].contains_changes);
    assert_eq!(rows[2].status, Some(ChangeStatus::Modified));
    assert_eq!(rows[2].depth, 1);

    assert!(!tree.toggle("src"));
    assert_eq!(paths(&tree, ""), ["src", "README.md"]);
}

#[test]
fn filter_keeps_ancestors_of_matches() {
    let mut tree = FileTree::default();
    tree.insert(listing(
        "",
        &[
            ("src", EntryKind::Directory),
            ("README.md", EntryKind::File),
        ],
    ));
    tree.insert(listing(
        "src",
        &[("main.rs", EntryKind::File), ("api.rs", EntryKind::File)],
    ));
    assert_eq!(paths(&tree, "MAIN"), ["src", "src/main.rs"]);
    // A directory matching by name keeps its own expansion state.
    assert_eq!(paths(&tree, "sr"), ["src"]);
    tree.reveal("src/api.rs");
    assert_eq!(paths(&tree, "sr"), ["src", "src/main.rs", "src/api.rs"]);
}

#[test]
fn languages_follow_names_and_extensions() {
    assert_eq!(language_for("src/main.rs"), Some("rust"));
    assert_eq!(language_for("Cargo.lock"), Some("toml"));
    assert_eq!(language_for("ui/app.JSX"), Some("javascript"));
    assert_eq!(language_for("notes.txt"), None);
    assert_eq!(language_for("LICENSE"), None);
    assert_eq!(split_path("a/b/c.rs"), ("a/b/", "c.rs"));
    assert_eq!(split_path("c.rs"), ("", "c.rs"));
}
