use super::*;

#[test]
fn workspace_git_refs_revision_tracks_create_move_delete_with_unchanged_head() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init(root);
    fs::write(root.join("tracked.txt"), "first\n").unwrap();
    git(root, &["add", "tracked.txt"]);
    git(root, &["commit", "-qm", "first"]);
    fs::write(root.join("tracked.txt"), "second\n").unwrap();
    git(root, &["add", "tracked.txt"]);
    git(root, &["commit", "-qm", "second"]);
    let before = changes(root);
    assert!(before["refs_revision"].is_string());

    git(root, &["update-ref", "refs/remotes/origin/new", "HEAD~1"]);
    let created = changes(root);
    assert_ne!(created["refs_revision"], before["refs_revision"]);
    assert_eq!(changes(root)["refs_revision"], created["refs_revision"]);
    git(root, &["pack-refs", "--all", "--prune"]);
    assert_eq!(changes(root)["refs_revision"], created["refs_revision"]);

    git(root, &["update-ref", "refs/remotes/origin/new", "HEAD"]);
    let moved = changes(root);
    assert_ne!(moved["refs_revision"], created["refs_revision"]);
    git(root, &["update-ref", "-d", "refs/remotes/origin/new"]);
    let deleted = changes(root);
    assert_ne!(deleted["refs_revision"], moved["refs_revision"]);
    assert_eq!(deleted["refs_revision"], before["refs_revision"]);
    for snapshot in [&created, &moved, &deleted] {
        assert_eq!(snapshot["head"], before["head"]);
        assert_eq!(snapshot["branch"], before["branch"]);
        assert_eq!(snapshot["entries"], before["entries"]);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_git_http_refuses_matching_clean_and_process_filters() {
    for kind in ["clean", "process"] {
        let (state, server, directory) = test_state().await;
        let root = directory.path();
        init(root);
        fs::write(root.join("tracked.txt"), "before\n").unwrap();
        git(root, &["add", "tracked.txt"]);
        git(root, &["commit", "-qm", "base"]);
        fs::write(root.join("tracked.txt"), "different contents\n").unwrap();
        let marker = root.join("filter-ran");
        let script = root.join("filter.sh");
        fs::write(
            &script,
            format!("printf called > '{}'\nexit 1\n", marker.display()),
        )
        .unwrap();
        git(
            root,
            &[
                "config",
                &format!("filter.probe.{kind}"),
                &format!("sh '{}'", script.display()),
            ],
        );
        git(root, &["config", "filter.probe.required", "true"]);

        for (attribute, expected) in [
            ("unknown", StatusCode::OK),
            ("probe", StatusCode::BAD_REQUEST),
        ] {
            fs::write(
                root.join(".gitattributes"),
                format!("tracked.txt filter={attribute}\n"),
            )
            .unwrap();
            for endpoint in ["/workspace/changes", "/workspace/diff"] {
                let mut uri = session_uri(endpoint, &server);
                if endpoint.ends_with("diff") {
                    uri.push_str("&path=tracked.txt");
                }
                let response = route_request(state.clone(), authed_get_request(&uri))
                    .await
                    .unwrap();
                assert_eq!(response.status(), expected, "{kind}/{attribute}/{endpoint}");
                let body = response_bytes(response).await;
                if attribute == "probe" {
                    assert!(
                        String::from_utf8_lossy(&body)
                            .contains("read-only Git refuses executable filter 'probe'"),
                        "{}",
                        String::from_utf8_lossy(&body)
                    );
                }
                assert!(
                    !marker.exists(),
                    "{kind}/{attribute}/{endpoint} executed the filter"
                );
            }
        }
        let nested = root.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("inside.txt"), "inside\n").unwrap();
        let nested_config = root.join("nested.config.toml");
        let nested_server =
            AgentAppServer::launch(crate::test_model::config(), nested, Some(&nested_config))
                .await
                .unwrap();
        let (shutdown, _) = tokio::sync::broadcast::channel(1);
        let nested_state =
            HttpAppState::new(nested_server.clone(), shutdown, test_security()).await;
        for endpoint in ["/workspace/changes", "/workspace/diff"] {
            let mut uri = session_uri(endpoint, &nested_server);
            if endpoint.ends_with("diff") {
                uri.push_str("&path=inside.txt");
            }
            let response = route_request(nested_state.clone(), authed_get_request(&uri))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "nested {kind}/{endpoint}"
            );
            let body = response_bytes(response).await;
            assert!(
                String::from_utf8_lossy(&body)
                    .contains("read-only Git refuses executable filter 'probe'"),
                "{}",
                String::from_utf8_lossy(&body)
            );
            assert!(
                !marker.exists(),
                "nested {kind}/{endpoint} executed the filter"
            );
        }
        nested_server.shutdown().await;
        server.shutdown().await;
        // Same repository, attributes and driver execute with an unguarded Git read.
        let _ = Command::new("git")
            .current_dir(root)
            .args([
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--",
                "tracked.txt",
            ])
            .output()
            .unwrap();
        assert!(
            marker.exists(),
            "{kind} fixture never exercised its executable driver"
        );
    }
}
