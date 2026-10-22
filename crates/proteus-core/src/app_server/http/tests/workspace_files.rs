use super::workspace_fixture::{commit_all, get, git, init, post};
use super::*;
use std::fs;

use super::super::workspace::files::{self, EntryKind};

#[test]
fn workspace_files_create_rename_and_delete_stay_inside_session() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("project");
    fs::create_dir(&root).unwrap();
    let outside = base.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep.txt"), "keep\n").unwrap();

    files::create(&root, "src/deep/main.rs", EntryKind::File).unwrap();
    assert_eq!(fs::read(root.join("src/deep/main.rs")).unwrap(), b"");
    files::create(&root, "docs", EntryKind::Directory).unwrap();
    assert!(root.join("docs").is_dir());
    for (path, kind) in [
        ("src/deep/main.rs", EntryKind::File),
        ("docs", EntryKind::Directory),
        ("../escape", EntryKind::File),
        ("/tmp/abs", EntryKind::File),
        ("", EntryKind::Directory),
        (".", EntryKind::Directory),
    ] {
        assert!(files::create(&root, path, kind).is_err(), "{path}");
    }

    files::rename(&root, "src/deep/main.rs", "src/lib.rs").unwrap();
    assert!(root.join("src/lib.rs").is_file() && !root.join("src/deep/main.rs").exists());
    files::rename(&root, "src", "moved/src").unwrap();
    assert!(root.join("moved/src/lib.rs").is_file());
    for (from, to) in [
        ("missing", "x"),
        ("moved", "docs"),
        ("moved", "moved/inner"),
        ("docs", "../docs"),
    ] {
        assert!(files::rename(&root, from, to).is_err(), "{from} -> {to}");
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        assert!(files::create(&root, "link/new.txt", EntryKind::File).is_err());
        assert!(files::delete(&root, &["link/keep.txt".to_owned()]).is_err());
        // Deleting the link removes the link only.
        files::delete(&root, &["link".to_owned()]).unwrap();
        assert!(!root.join("link").exists() && outside.join("keep.txt").exists());
    }
    files::delete(&root, &["moved".to_owned(), "moved/src/lib.rs".to_owned()]).unwrap();
    assert!(!root.join("moved").exists());
    for paths in [
        vec![],
        vec!["".to_owned()],
        vec![".".to_owned()],
        vec!["../outside".to_owned()],
        vec!["gone".to_owned()],
    ] {
        assert!(files::delete(&root, &paths).is_err(), "{paths:?}");
    }
    assert!(root.exists() && outside.join("keep.txt").exists());
}

#[tokio::test]
async fn workspace_new_http_endpoints_require_auth_and_strict_bodies() {
    let (state, server, root) = test_state().await;
    init(root.path());
    fs::write(root.path().join("file.txt"), "one\n").unwrap();
    commit_all(root.path(), "first");
    let hash = git(root.path(), &["rev-parse", "HEAD"]).trim().to_owned();
    let session_dir = server.session_dir_path().unwrap().display().to_string();
    for endpoint in [
        "/workspace/fetch",
        "/workspace/pull",
        "/workspace/push",
        "/workspace/uncommit",
        "/workspace/stash",
        "/workspace/stash/pop",
        "/workspace/ignore",
        "/workspace/create",
        "/workspace/rename",
        "/workspace/delete",
    ] {
        let unauthorized = route_request(
            state.clone(),
            Request::builder()
                .method(Method::POST)
                .uri(endpoint)
                .body(json_body(json!({"session_dir": session_dir})))
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(
            unauthorized.status(),
            StatusCode::UNAUTHORIZED,
            "{endpoint}"
        );
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
        "/workspace/create",
        json!({"session_dir": session_dir, "path": "dir/new.txt", "kind": "file"}),
    )
    .await;
    assert_eq!((status, &body), (StatusCode::OK, &json!({"ok": true})));
    let (status, _) = post(
        &state,
        "/workspace/rename",
        json!({"session_dir": session_dir, "from": "dir/new.txt", "to": "renamed.txt"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = post(
        &state,
        "/workspace/ignore",
        json!({"session_dir": session_dir, "paths": ["renamed.txt"], "target": "exclude"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["path"] != "renamed.txt"),
        "{body}"
    );
    let (status, _) = post(
        &state,
        "/workspace/delete",
        json!({"session_dir": session_dir, "paths": ["renamed.txt", "dir"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = post(
        &state,
        "/workspace/fetch",
        json!({"session_dir": session_dir}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["repository"], true);
    let (status, body) = post(
        &state,
        "/workspace/uncommit",
        json!({"session_dir": session_dir}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"].as_str().unwrap().contains("no parent"),
        "{body}"
    );
    let (status, body) = post(
        &state,
        "/workspace/checkout",
        json!({"session_dir": session_dir, "branch": "created", "create": true}),
    )
    .await;
    assert_eq!(
        (status, &body["branch"]),
        (StatusCode::OK, &json!("created"))
    );

    let short = &hash[..8];
    for (endpoint, ok) in [
        ("/workspace/log?limit=500&skip=0&all=true".to_owned(), true),
        ("/workspace/log?limit=501".to_owned(), false),
        ("/workspace/log?all=yes".to_owned(), false),
        (format!("/workspace/commit?hash={short}"), true),
        ("/workspace/commit?hash=xyz1".to_owned(), false),
        ("/workspace/commit".to_owned(), false),
        (
            format!("/workspace/commit/diff?hash={hash}&path=file.txt"),
            true,
        ),
        (format!("/workspace/commit/diff?hash={hash}"), false),
        (format!("/workspace/commit?hash={hash}&path=x"), false),
    ] {
        let (status, body) = get(&state, &server, &endpoint).await;
        let expected = if ok {
            StatusCode::OK
        } else {
            StatusCode::BAD_REQUEST
        };
        assert_eq!(status, expected, "{endpoint}: {body}");
    }
    let (_, body) = get(&state, &server, &format!("/workspace/commit?hash={short}")).await;
    assert_eq!(
        (&body["hash"], &body["summary"]),
        (&json!(hash), &json!("first"))
    );
    let (_, body) = get(
        &state,
        &server,
        &format!("/workspace/commit/diff?hash={hash}&path=file.txt"),
    )
    .await;
    assert_eq!(body["kind"], "text");
    server.shutdown().await;
}
