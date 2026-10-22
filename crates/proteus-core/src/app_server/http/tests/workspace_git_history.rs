use super::workspace_fixture::{commit_all, entry, git, init, json};
use super::*;
use std::fs;

use super::super::workspace::git::{self as workspace_git, LogQuery};

fn log(root: &std::path::Path, limit: usize, skip: usize, all: bool) -> Value {
    json(workspace_git::log(root, LogQuery { limit, skip, all }).unwrap())
}

#[test]
fn workspace_git_changes_count_lines_against_head() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    init(root);
    fs::write(root.join("edited.txt"), "a\nb\nc\n").unwrap();
    fs::write(root.join("old.txt"), "same\nlines\n").unwrap();
    fs::write(root.join("binary"), [0, 1, 2]).unwrap();
    commit_all(root, "base");
    // Staged and unstaged edits of one file are counted together.
    fs::write(root.join("edited.txt"), "a\nB\nc\nd\n").unwrap();
    git(root, &["add", "edited.txt"]);
    fs::write(root.join("edited.txt"), "a\nB\nc\nd\ne\n").unwrap();
    git(root, &["mv", "old.txt", "new.txt"]);
    fs::write(root.join("binary"), [0, 3]).unwrap();
    fs::write(root.join("untracked.txt"), "one\ntwo\nno newline").unwrap();
    fs::write(root.join("untracked.bin"), [7, 0, 7]).unwrap();
    let body = json(workspace_git::changes(root).unwrap());
    let counts = |path: &str| {
        let entry = entry(&body, path);
        (entry["added"].clone(), entry["deleted"].clone())
    };
    assert_eq!(counts("edited.txt"), (json!(3), json!(1)));
    assert_eq!(counts("new.txt"), (json!(0), json!(0)));
    assert_eq!(counts("binary"), (Value::Null, Value::Null));
    assert_eq!(counts("untracked.txt"), (json!(3), json!(0)));
    assert_eq!(counts("untracked.bin"), (Value::Null, Value::Null));
    assert_eq!((&body["added"], &body["deleted"]), (&json!(6), &json!(1)));

    // Without commits everything tracked is counted against the empty tree.
    let unborn = tempfile::tempdir().unwrap();
    init(unborn.path());
    fs::write(unborn.path().join("staged.txt"), "x\ny\n").unwrap();
    git(unborn.path(), &["add", "staged.txt"]);
    let body = json(workspace_git::changes(unborn.path()).unwrap());
    assert_eq!(entry(&body, "staged.txt")["added"], 2);
}

#[test]
fn workspace_git_log_pages_graph_parents_and_refs() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    init(root);
    assert_eq!(
        log(root, 50, 0, false),
        json!({"head": null, "entries": [], "has_more": false})
    );
    fs::write(root.join("file.txt"), "one\n").unwrap();
    commit_all(root, "first");
    git(root, &["tag", "v1"]);
    git(root, &["switch", "-q", "-c", "side"]);
    fs::write(root.join("side.txt"), "side\n").unwrap();
    commit_all(root, "side work");
    git(root, &["switch", "-q", "main"]);
    fs::write(root.join("file.txt"), "two\n").unwrap();
    commit_all(root, "second");
    git(root, &["update-ref", "refs/remotes/origin/main", "HEAD~1"]);
    let head = git(root, &["rev-parse", "HEAD"]).trim().to_owned();
    let first = git(root, &["rev-parse", "HEAD~1"]).trim().to_owned();

    let body = log(root, 50, 0, false);
    assert_eq!(body["head"], head);
    assert_eq!(body["has_more"], false);
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{body}");
    assert_eq!(entries[0]["summary"], "second");
    assert_eq!(entries[0]["parents"], json!([first]));
    assert_eq!(entries[0]["email"], "fixture@example.invalid");
    assert_eq!(
        entries[0]["refs"],
        json!([{"name": "HEAD", "kind": "head"}, {"name": "main", "kind": "branch"}])
    );
    assert_eq!(entries[1]["parents"], json!([]));
    let refs = entries[1]["refs"].as_array().unwrap();
    for expected in [
        json!({"name": "v1", "kind": "tag"}),
        json!({"name": "origin/main", "kind": "remote"}),
    ] {
        assert!(refs.contains(&expected), "{refs:?}");
    }

    let all = log(root, 50, 0, true);
    let summaries: Vec<_> = all["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["summary"].as_str().unwrap())
        .collect();
    assert_eq!(summaries.len(), 3);
    assert!(summaries.contains(&"side work"));
    assert_eq!(summaries[2], "first");

    let page = log(root, 1, 0, true);
    assert_eq!(page["has_more"], true);
    assert_eq!(page["entries"].as_array().unwrap().len(), 1);
    let last = log(root, 2, 1, true);
    assert_eq!(last["has_more"], false);
    assert_eq!(last["entries"].as_array().unwrap().len(), 2);
}

#[test]
fn workspace_git_commit_details_and_file_diff() {
    let repo = tempfile::tempdir().unwrap();
    init(repo.path());
    let root = repo.path().join("sub");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("a.txt"), "one\n").unwrap();
    fs::write(repo.path().join("top.txt"), "top\n").unwrap();
    commit_all(repo.path(), "root commit");
    fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
    fs::write(repo.path().join("top.txt"), "changed\n").unwrap();
    fs::write(root.join("bin"), [0, 1]).unwrap();
    git(repo.path(), &["add", "-A"]);
    git(
        repo.path(),
        &["commit", "-qm", "second", "-m", "Body text."],
    );
    let hash = git(repo.path(), &["rev-parse", "HEAD"]).trim().to_owned();
    let parent = git(repo.path(), &["rev-parse", "HEAD~1"]).trim().to_owned();

    let body = json(workspace_git::commit_details(&root, &hash[..10]).unwrap());
    assert_eq!(body["hash"], hash);
    assert_eq!(body["summary"], "second");
    assert_eq!(body["body"], "Body text.");
    assert_eq!(body["email"], "fixture@example.invalid");
    assert_eq!(body["parents"], json!([parent]));
    assert_eq!(body["prefix"], "sub/");
    assert_eq!(body["truncated"], false);
    let file = entry(&json!({"entries": body["files"]}), "sub/a.txt");
    assert_eq!(
        (&file["status"], &file["added"], &file["deleted"]),
        (&json!("modified"), &json!(1), &json!(0))
    );
    let binary = entry(&json!({"entries": body["files"]}), "sub/bin");
    assert_eq!(
        (&binary["status"], &binary["added"]),
        (&json!("added"), &Value::Null)
    );
    // Files outside the session cwd belong to the commit too.
    entry(&json!({"entries": body["files"]}), "top.txt");
    assert!(body["patch"].as_str().unwrap().contains("+two\n"));

    let root_commit = json(workspace_git::commit_details(&root, &parent).unwrap());
    assert_eq!(root_commit["parents"], json!([]));
    assert_eq!(root_commit["files"].as_array().unwrap().len(), 2);

    let diff = json(workspace_git::commit_diff(&root, &hash, "sub/a.txt".to_owned()).unwrap());
    assert_eq!(diff["kind"], "text");
    let patch = diff["patch"].as_str().unwrap();
    assert!(patch.contains("+two\n") && !patch.contains("top.txt"));
    let binary = json(workspace_git::commit_diff(&root, &hash, "sub/bin".to_owned()).unwrap());
    assert_eq!(binary["kind"], "binary");
    let untouched = json(workspace_git::commit_diff(&root, &parent, "missing".to_owned()).unwrap());
    assert_eq!(untouched["kind"], "unavailable");
    for bad in ["abc", "zzzz", "--all", &"a".repeat(65), "deadbeef"] {
        assert!(workspace_git::commit_details(&root, bad).is_err(), "{bad}");
    }
    for path in ["../x", "/etc/passwd", ""] {
        assert!(workspace_git::commit_diff(&root, &hash, path.to_owned()).is_err());
    }
}

#[test]
fn workspace_git_branches_report_last_commit() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    init(root);
    fs::write(root.join("file.txt"), "one\n").unwrap();
    commit_all(root, "latest subject");
    let body = json(workspace_git::branches(root).unwrap());
    let branch = &body["branches"][0];
    assert_eq!(branch["name"], "main");
    assert_eq!(branch["last_commit_summary"], "latest subject");
    assert_eq!(branch["author"], "Workspace fixture");
}
