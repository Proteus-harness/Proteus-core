use super::workspace_fixture::{commit_all, configure, entry, git, init, json};
use super::*;
use std::fs;

use super::super::workspace::git::{self as workspace_git, CommitOptions, IgnoreTarget};

/// A bare remote with one commit, and two independent clones of it.
fn remote_pair() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let base = tempfile::tempdir().unwrap();
    let remote = base.path().join("remote.git");
    let seed = base.path().join("seed");
    fs::create_dir(&seed).unwrap();
    git(
        base.path(),
        &["init", "-q", "--bare", "-b", "main", "remote.git"],
    );
    init(&seed);
    fs::write(seed.join("file.txt"), "base\n").unwrap();
    commit_all(&seed, "base");
    git(
        &seed,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&seed, &["push", "-q", "-u", "origin", "main"]);
    git(&seed, &["config", "push.autoSetupRemote", "false"]);
    let other = base.path().join("other");
    git(
        base.path(),
        &["clone", "-q", remote.to_str().unwrap(), "other"],
    );
    configure(&other);
    (base, seed, other)
}

#[test]
fn workspace_git_fetch_pull_and_push_sync_with_remote() {
    let (_base, root, other) = remote_pair();
    fs::write(other.join("file.txt"), "theirs\n").unwrap();
    commit_all(&other, "theirs");
    git(&other, &["push", "-q"]);

    let fetched = json(workspace_git::fetch(&root).unwrap());
    assert_eq!(
        (&fetched["ahead"], &fetched["behind"]),
        (&json!(0), &json!(1))
    );
    let pulled = json(workspace_git::pull(&root, Some(false)).unwrap());
    assert_eq!(pulled["behind"], 0);
    assert_eq!(
        fs::read_to_string(root.join("file.txt")).unwrap(),
        "theirs\n"
    );

    fs::write(root.join("file.txt"), "mine\n").unwrap();
    commit_all(&root, "mine");
    let pushed = json(workspace_git::push(&root, false, false).unwrap());
    assert_eq!(pushed["ahead"], 0);
    git(&other, &["pull", "-q", "--no-rebase"]);
    assert_eq!(
        fs::read_to_string(other.join("file.txt")).unwrap(),
        "mine\n"
    );

    // A rewritten branch needs force; the lease protects unseen remote work.
    git(&root, &["commit", "-q", "--amend", "-m", "rewritten"]);
    let error = workspace_git::push(&root, false, false)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("failed to push"), "{error}");
    workspace_git::push(&root, true, false).unwrap();

    git(&root, &["switch", "-q", "-c", "topic"]);
    let error = workspace_git::push(&root, false, false)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("no upstream"), "{error}");
    let published = json(workspace_git::push(&root, false, true).unwrap());
    assert_eq!(published["upstream"], "origin/topic");

    fs::write(other.join("file.txt"), "rebased\n").unwrap();
    commit_all(&other, "remote change");
    git(&other, &["push", "-q", "--force"]);
    fs::write(root.join("local.txt"), "local\n").unwrap();
    git(&root, &["switch", "-q", "main"]);
    git(&root, &["reset", "-q", "--hard", "origin/main"]);
    git(&root, &["add", "local.txt"]);
    git(&root, &["commit", "-qm", "local"]);
    let rebased = json(workspace_git::pull(&root, Some(true)).unwrap());
    assert_eq!(
        (&rebased["ahead"], &rebased["behind"]),
        (&json!(1), &json!(0))
    );
}

#[test]
fn workspace_git_stash_uncommit_and_commit_options() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    init(root);
    fs::write(root.join("file.txt"), "base\n").unwrap();
    commit_all(root, "base");
    let error = workspace_git::uncommit(root).err().unwrap().to_string();
    assert!(error.contains("no parent"), "{error}");
    let error = workspace_git::stash(root, None, false)
        .err()
        .unwrap()
        .to_string();
    assert_eq!(error, "no local changes to stash");

    fs::write(root.join("file.txt"), "edited\n").unwrap();
    fs::write(root.join("new.txt"), "new\n").unwrap();
    let stashed = json(workspace_git::stash(root, Some("wip"), false).unwrap());
    assert_eq!(stashed["entries"], json!([]));
    assert!(git(root, &["stash", "list"]).contains("wip"));
    let popped = json(workspace_git::stash_pop(root).unwrap());
    assert_eq!(entry(&popped, "new.txt")["status"], "untracked");
    assert!(workspace_git::stash_pop(root).is_err());

    git(root, &["add", "new.txt"]);
    workspace_git::stash(root, None, true).unwrap();
    assert!(!root.join("new.txt").exists());
    assert_eq!(
        fs::read_to_string(root.join("file.txt")).unwrap(),
        "edited\n"
    );
    workspace_git::stash_pop(root).unwrap();

    fs::write(
        root.join(".git/hooks/pre-commit"),
        "#!/bin/sh\necho hook refused >&2\nexit 1\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            root.join(".git/hooks/pre-commit"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let error = workspace_git::commit(root, "blocked", CommitOptions::default())
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("hook refused"), "{error}");
    }
    // `new.txt` is staged; `file.txt` is only modified until all_tracked.
    let options = CommitOptions {
        signoff: true,
        no_verify: true,
        all_tracked: true,
        ..CommitOptions::default()
    };
    let committed = json(workspace_git::commit(root, "tracked", options).unwrap());
    assert_eq!(committed["changes"]["entries"], json!([]));
    let message = git(root, &["log", "-1", "--format=%B"]);
    assert!(
        message.contains("Signed-off-by: Workspace fixture"),
        "{message}"
    );

    let uncommitted = json(workspace_git::uncommit(root).unwrap());
    assert_eq!(entry(&uncommitted, "file.txt")["staged"], "modified");
    assert_eq!(uncommitted["head"]["summary"], "base");
}

#[test]
fn workspace_git_checkout_create_and_ignore() {
    let repo = tempfile::tempdir().unwrap();
    init(repo.path());
    let root = repo.path().join("sub");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file.txt"), "base\n").unwrap();
    commit_all(repo.path(), "base");

    let created = json(workspace_git::checkout(&root, "feature/x", true).unwrap());
    assert_eq!(created["branch"], "feature/x");
    for bad in ["feature/x", "bad..name", "-x"] {
        assert!(workspace_git::checkout(&root, bad, true).is_err(), "{bad}");
    }

    fs::write(root.join("build.log"), "").unwrap();
    fs::write(root.join("a*b"), "").unwrap();
    fs::write(root.join("local.tmp"), "").unwrap();
    let paths = |list: &[&str]| list.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>();
    let ignored = json(
        workspace_git::ignore(
            &root,
            &paths(&["build.log", "a*b"]),
            IgnoreTarget::Gitignore,
        )
        .unwrap(),
    );
    let listed = |changes: &Value| -> Vec<String> {
        changes["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["path"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(listed(&ignored), ["local.tmp"]);
    workspace_git::ignore(&root, &paths(&["build.log"]), IgnoreTarget::Gitignore).unwrap();
    assert_eq!(
        fs::read_to_string(repo.path().join(".gitignore")).unwrap(),
        "/sub/build.log\n/sub/a\\*b\n"
    );
    let excluded =
        json(workspace_git::ignore(&root, &paths(&["local.tmp"]), IgnoreTarget::Exclude).unwrap());
    assert!(listed(&excluded).is_empty(), "{excluded}");
    let exclude = fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap();
    assert!(exclude.ends_with("\n/sub/local.tmp\n") || exclude == "/sub/local.tmp\n");
    for bad in ["../x", "", "/etc"] {
        assert!(workspace_git::ignore(&root, &paths(&[bad]), IgnoreTarget::Exclude).is_err());
    }
    assert!(workspace_git::ignore(&root, &[], IgnoreTarget::Exclude).is_err());
}
