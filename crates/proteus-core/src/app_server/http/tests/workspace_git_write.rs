use super::*;
use std::{fs, path::Path, process::Command};

use super::super::workspace::git::{self as workspace_git, Selection};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn init(root: &Path) {
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.name", "Workspace fixture"]);
    git(root, &["config", "user.email", "fixture@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
}

fn commit_all(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", message]);
}

fn json<T: serde::Serialize>(value: T) -> Value {
    serde_json::to_value(value).unwrap()
}

fn entry(changes: &Value, path: &str) -> Value {
    changes["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == path)
        .cloned()
        .unwrap_or_else(|| panic!("{path} missing in {changes}"))
}

fn paths(list: &[&str]) -> Selection {
    Selection::new(list.iter().map(|p| (*p).to_owned()).collect(), false).unwrap()
}

#[test]
fn workspace_git_changes_report_branch_head_and_index_sides() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    init(root);
    let unborn = json(workspace_git::changes(root).unwrap());
    assert_eq!(
        (&unborn["branch"], &unborn["head"]),
        (&json!("main"), &Value::Null)
    );
    for name in ["edited.txt", "old.txt", "added-later.txt"] {
        fs::write(root.join(name), "base\n").unwrap();
    }
    commit_all(root, "base");
    git(root, &["switch", "-q", "-c", "feature"]);
    git(root, &["branch", "-q", "-u", "main"]);
    fs::write(root.join("edited.txt"), "ahead\n").unwrap();
    commit_all(root, "ahead commit");
    fs::write(root.join("edited.txt"), "staged\n").unwrap();
    git(root, &["add", "edited.txt"]);
    fs::write(root.join("edited.txt"), "working\n").unwrap();
    git(root, &["mv", "old.txt", "renamed.txt"]);
    fs::write(root.join("new.txt"), "new\n").unwrap();
    git(root, &["add", "new.txt"]);
    fs::write(root.join("new.txt"), "changed\n").unwrap();
    fs::write(root.join("untracked.txt"), "u\n").unwrap();
    let body = json(workspace_git::changes(root).unwrap());
    assert_eq!(body["branch"], "feature");
    assert_eq!(body["upstream"], "main");
    assert_eq!((&body["ahead"], &body["behind"]), (&json!(1), &json!(0)));
    assert_eq!(body["head"]["summary"], "ahead commit");
    assert!(!body["head"]["short"].as_str().unwrap().is_empty());
    let expected = [
        (
            "edited.txt",
            "modified",
            json!("modified"),
            json!("modified"),
        ),
        ("renamed.txt", "renamed", json!("renamed"), Value::Null),
        ("new.txt", "added", json!("added"), json!("modified")),
        (
            "untracked.txt",
            "untracked",
            Value::Null,
            json!("untracked"),
        ),
    ];
    for (path, status, staged, unstaged) in expected {
        let entry = entry(&body, path);
        assert_eq!(entry["status"], status, "{entry}");
        assert_eq!((&entry["staged"], &entry["unstaged"]), (&staged, &unstaged));
    }
    assert_eq!(entry(&body, "renamed.txt")["original"], "old.txt");
    assert_eq!(entry(&body, "edited.txt")["original"], Value::Null);

    git(root, &["stash", "-q", "-u"]);
    git(root, &["switch", "-q", "main"]);
    fs::write(root.join("edited.txt"), "theirs\n").unwrap();
    commit_all(root, "theirs");
    git(root, &["switch", "-q", "feature"]);
    let diverged = json(workspace_git::changes(root).unwrap());
    assert_eq!(
        (&diverged["ahead"], &diverged["behind"]),
        (&json!(1), &json!(1))
    );
    let merge = Command::new("git")
        .current_dir(root)
        .args(["merge", "-q", "main"])
        .output()
        .unwrap();
    assert!(!merge.status.success());
    let conflict = entry(&json(workspace_git::changes(root).unwrap()), "edited.txt");
    assert_eq!(
        (
            &conflict["status"],
            &conflict["staged"],
            &conflict["unstaged"]
        ),
        (&json!("conflict"), &json!("conflict"), &json!("conflict"))
    );
    git(root, &["merge", "--abort"]);
    git(root, &["switch", "-q", "--detach"]);
    let detached = json(workspace_git::changes(root).unwrap());
    assert_eq!(
        (&detached["branch"], &detached["upstream"]),
        (&Value::Null, &Value::Null)
    );
    assert_eq!(detached["head"]["summary"], "ahead commit");
}

#[test]
fn workspace_git_stage_unstage_and_discard_stay_inside_session() {
    let repo = tempfile::tempdir().unwrap();
    init(repo.path());
    let root = repo.path().join("sub");
    fs::create_dir_all(root.join("dir")).unwrap();
    fs::write(repo.path().join("outside.txt"), "base\n").unwrap();
    fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
    for name in ["tracked.txt", "old.txt", "dir/kept.txt"] {
        fs::write(root.join(name), "base\n").unwrap();
    }
    commit_all(repo.path(), "base");
    fs::write(repo.path().join("outside.txt"), "outside\n").unwrap();
    fs::write(root.join("tracked.txt"), "changed\n").unwrap();
    fs::write(root.join("ignored.txt"), "keep\n").unwrap();
    fs::write(root.join("dir/kept.txt"), "changed\n").unwrap();
    fs::write(root.join("dir/new.txt"), "new\n").unwrap();
    git(&root, &["mv", "old.txt", "renamed.txt"]);
    let outside_target = tempfile::tempdir().unwrap();
    fs::write(outside_target.path().join("target"), "keep\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside_target.path().join("target"), root.join("link")).unwrap();

    let staged = json(workspace_git::stage(&root, paths(&["tracked.txt", "dir"])).unwrap());
    assert_eq!(entry(&staged, "tracked.txt")["staged"], "modified");
    assert_eq!(entry(&staged, "dir/new.txt")["staged"], "added");
    assert_eq!(entry(&staged, "dir/new.txt")["unstaged"], Value::Null);
    let unstaged = json(workspace_git::unstage(&root, paths(&["dir/new.txt"])).unwrap());
    assert_eq!(entry(&unstaged, "dir/new.txt")["unstaged"], "untracked");
    // Unstaging a rename also restores its source in the index.
    let unrenamed = json(workspace_git::unstage(&root, paths(&["renamed.txt"])).unwrap());
    assert_eq!(entry(&unrenamed, "old.txt")["unstaged"], "deleted");
    assert_eq!(entry(&unrenamed, "renamed.txt")["unstaged"], "untracked");
    git(&root, &["add", "-A", "old.txt", "renamed.txt"]);
    for path in ["../outside.txt", "/etc/passwd", "", "."] {
        assert!(
            workspace_git::stage(&root, paths(&[path])).is_err(),
            "{path}"
        );
        assert!(
            workspace_git::discard(&root, paths(&[path])).is_err(),
            "{path}"
        );
    }
    assert!(Selection::new(Vec::new(), false).is_err());
    assert!(Selection::new(vec!["a".to_owned()], true).is_err());

    let discarded = json(workspace_git::discard(&root, paths(&["renamed.txt"])).unwrap());
    assert!(
        discarded["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| { e["path"] != "renamed.txt" && e["path"] != "old.txt" })
    );
    assert_eq!(fs::read_to_string(root.join("old.txt")).unwrap(), "base\n");
    let clean =
        json(workspace_git::discard(&root, Selection::new(Vec::new(), true).unwrap()).unwrap());
    assert_eq!(clean["entries"], json!([]), "{clean}");
    assert_eq!(
        fs::read_to_string(root.join("tracked.txt")).unwrap(),
        "base\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("dir/kept.txt")).unwrap(),
        "base\n"
    );
    assert!(!root.join("dir/new.txt").exists() && !root.join("link").exists());
    assert!(root.join("ignored.txt").exists());
    assert!(outside_target.path().join("target").exists());
    assert_eq!(
        fs::read_to_string(repo.path().join("outside.txt")).unwrap(),
        "outside\n"
    );
}

#[test]
fn workspace_git_unborn_repository_unstages_and_discards_index_entries() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    init(root);
    fs::write(root.join("a.txt"), "a\n").unwrap();
    fs::write(root.join("b.txt"), "b\n").unwrap();
    workspace_git::stage(root, Selection::new(Vec::new(), true).unwrap()).unwrap();
    fs::write(root.join("a.txt"), "edited\n").unwrap();
    let body = json(workspace_git::unstage(root, paths(&["a.txt"])).unwrap());
    assert_eq!(entry(&body, "a.txt")["unstaged"], "untracked");
    assert_eq!(entry(&body, "b.txt")["staged"], "added");
    let body = json(workspace_git::discard(root, paths(&["b.txt"])).unwrap());
    assert!(!root.join("b.txt").exists());
    assert_eq!(body["entries"].as_array().unwrap().len(), 1);
}

#[test]
fn workspace_git_commit_log_branches_and_checkout() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    init(root);
    fs::write(root.join("file.txt"), "one\n").unwrap();
    let error = workspace_git::commit(root, "first", false).err().unwrap();
    assert_eq!(error.to_string(), "nothing is staged to commit");
    assert!(workspace_git::commit(root, "  \n", false).is_err());
    workspace_git::stage(root, paths(&["file.txt"])).unwrap();
    let first = json(workspace_git::commit(root, "  first\n", false).unwrap());
    assert_eq!(first["commit"]["summary"], "first");
    assert_eq!(first["changes"]["entries"], json!([]));
    assert_eq!(first["changes"]["head"], first["commit"]);
    let amended = json(workspace_git::commit(root, "first, amended", true).unwrap());
    assert_eq!(amended["commit"]["summary"], "first, amended");

    git(root, &["switch", "-q", "-c", "topic"]);
    fs::write(root.join("file.txt"), "two\n").unwrap();
    commit_all(root, "second");
    git(root, &["branch", "-q", "-u", "main"]);
    let log = json(workspace_git::log(root, 20).unwrap());
    let entries = log["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["summary"], "second");
    assert_eq!(entries[0]["author"], "Workspace fixture");
    assert_eq!(entries[0]["hash"], git(root, &["rev-parse", "HEAD"]).trim());
    assert!(entries[0]["short"].as_str().unwrap().len() < 40);
    assert!(entries[0]["time"].as_i64().unwrap() > 0);
    assert_eq!(
        json(workspace_git::log(root, 1).unwrap())["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for limit in [0, 101] {
        assert!(workspace_git::log(root, limit).is_err());
    }

    let branches = json(workspace_git::branches(root).unwrap());
    assert_eq!(branches["current"], "topic");
    let names: Vec<_> = branches["branches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"topic".to_owned()) && names.contains(&"main".to_owned()));
    let topic = &branches["branches"][names.iter().position(|n| n == "topic").unwrap()];
    assert_eq!(topic["upstream"], "main");
    assert!(topic["last_commit_time"].as_i64().unwrap() > 0);

    let switched = json(workspace_git::checkout(root, "main").unwrap());
    assert_eq!(switched["branch"], "main");
    assert_eq!(fs::read_to_string(root.join("file.txt")).unwrap(), "one\n");
    for branch in ["missing", "-f", ""] {
        assert!(workspace_git::checkout(root, branch).is_err(), "{branch}");
    }
    fs::write(root.join("file.txt"), "dirty\n").unwrap();
    let error = workspace_git::checkout(root, "topic")
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("file.txt"), "{error}");
}

async fn post(state: &HttpAppState, path: &str, body: Value) -> (StatusCode, Value) {
    let response = route_request(state.clone(), authed_json_request(path, body))
        .await
        .unwrap();
    let status = response.status();
    let body = serde_json::from_slice(&response_bytes(response).await).unwrap();
    (status, body)
}

#[tokio::test]
async fn workspace_git_write_http_requires_auth_and_strict_bodies() {
    let (state, server, root) = test_state().await;
    init(root.path());
    fs::write(root.path().join("file.txt"), "new\n").unwrap();
    let session_dir = server.session_dir_path().unwrap().display().to_string();
    for endpoint in [
        "/workspace/stage",
        "/workspace/unstage",
        "/workspace/discard",
        "/workspace/commit",
        "/workspace/checkout",
    ] {
        let unauthorized = route_request(
            state.clone(),
            Request::builder()
                .method(Method::POST)
                .uri(endpoint)
                .body(json_body(json!({"session_dir": session_dir, "all": true})))
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        let (status, _) = post(
            &state,
            endpoint,
            json!({"session_dir": session_dir, "extra": 1}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{endpoint}");
    }
    let (status, body) = post(
        &state,
        "/workspace/stage",
        json!({"session_dir": session_dir, "paths": ["file.txt"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(entry(&body, "file.txt")["staged"], "added");
    let (status, body) = post(
        &state,
        "/workspace/commit",
        json!({"session_dir": session_dir, "message": "from http"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["commit"]["summary"], "from http");
    assert_eq!(body["changes"]["branch"], "main");
    let (status, body) = post(
        &state,
        "/workspace/commit",
        json!({"session_dir": session_dir, "message": "again"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "nothing is staged to commit");
    let (status, body) = post(
        &state,
        "/workspace/checkout",
        json!({"session_dir": session_dir, "branch": "missing"}),
    )
    .await;
    assert_eq!(
        (status, &body["error"]),
        (
            StatusCode::BAD_REQUEST,
            &json!("unknown local branch: missing")
        )
    );

    for (endpoint, ok) in [
        ("/workspace/log?limit=5", true),
        ("/workspace/log?limit=0", false),
        ("/workspace/branches", true),
        ("/workspace/changes?path=x", false),
        ("/workspace/branches?extra=1", false),
    ] {
        let response = route_request(
            state.clone(),
            authed_get_request(&session_uri(endpoint, &server)),
        )
        .await
        .unwrap();
        let expected = if ok {
            StatusCode::OK
        } else {
            StatusCode::BAD_REQUEST
        };
        assert_eq!(response.status(), expected, "{endpoint}");
    }
    server.shutdown().await;
}
