//! Temporary Git repositories shared by the newer workspace tests.
use super::*;
use std::{path::Path, process::Command};

pub(super) fn git(root: &Path, args: &[&str]) -> String {
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

pub(super) fn init(root: &Path) {
    git(root, &["init", "-q", "-b", "main"]);
    configure(root);
}

pub(super) fn configure(root: &Path) {
    git(root, &["config", "user.name", "Workspace fixture"]);
    git(root, &["config", "user.email", "fixture@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
}

pub(super) fn commit_all(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", message]);
}

pub(super) fn json<T: serde::Serialize>(value: T) -> Value {
    serde_json::to_value(value).unwrap()
}

pub(super) fn entry(changes: &Value, path: &str) -> Value {
    changes["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == path)
        .cloned()
        .unwrap_or_else(|| panic!("{path} missing in {changes}"))
}

pub(super) async fn post(state: &HttpAppState, path: &str, body: Value) -> (StatusCode, Value) {
    let response = route_request(state.clone(), authed_json_request(path, body))
        .await
        .unwrap();
    let status = response.status();
    let body = serde_json::from_slice(&response_bytes(response).await).unwrap();
    (status, body)
}

pub(super) async fn get(
    state: &HttpAppState,
    server: &AppServerHandle,
    path: &str,
) -> (StatusCode, Value) {
    let response = route_request(
        state.clone(),
        authed_get_request(&session_uri(path, server)),
    )
    .await
    .unwrap();
    let status = response.status();
    let body = serde_json::from_slice(&response_bytes(response).await).unwrap();
    (status, body)
}
