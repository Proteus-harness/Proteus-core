//! Approval must remain interactive while a direct Remember owns its execution.
use super::*;
use std::collections::HashMap;

fn memory_config(home: &std::path::Path) -> (PathBuf, PathBuf) {
    let config = home.join("remember.toml");
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let record = home.join("memory.jsonl");
    let extra = format!(
        r#"
[modules]
policy = "probe.policy"
[tools]
enabled = ["remember_fact"]
[components.approval-policy]
command = "python3"
args = [{}]
[components.approval-policy.exports.policy."probe.policy"]
[components.phase8-memory-component]
command = "python3"
args = [{}]
[components.phase8-memory-component.exports.tool.phase8-memory]
[module_config.tool.phase8-memory]
record_path = {}
"#,
        serde_json::to_string(&fixtures.join("process_commands.py")).unwrap(),
        serde_json::to_string(&fixtures.join("process_phase8_memory.py")).unwrap(),
        serde_json::to_string(&record).unwrap()
    );
    std::fs::write(&config, extra).unwrap();
    (config, record)
}

async fn approval(peer: &mut Peer) -> String {
    loop {
        if let StdioOutput::Event { event } = peer.next().await
            && let AppServerEvent::ApprovalRequested { request } = *event
        {
            return request.approval_id;
        }
    }
}

async fn responses(peer: &mut Peer, ids: &[&str]) -> HashMap<String, bool> {
    let mut found = HashMap::new();
    while found.len() < ids.len() {
        if let StdioOutput::Response {
            id: Some(id), ok, ..
        } = peer.next().await
            && ids.contains(&id.as_str())
        {
            found.insert(id, ok);
        }
    }
    found
}

async fn remember(peer: &mut Peer) {
    peer.send(StdioRequest::Remember {
        id: Some("remember".into()),
        kind: "fact".into(),
        content: "synthetic remembered value".into(),
    })
    .await;
}

#[tokio::test]
async fn direct_remember_accepts_approval_and_denial_without_blocking_rotation() {
    for approved in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let (config, record) = memory_config(home.path());
        let mut peer = Peer::launch(&config, workspace.path());
        let first = peer.identity().await;
        remember(&mut peer).await;
        let approval_id = approval(&mut peer).await;
        assert!(!record.exists());
        peer.send(StdioRequest::NewSession {
            id: Some("busy".into()),
        })
        .await;
        assert!(!peer.response("busy").await.0);
        peer.send(StdioRequest::Approval {
            id: Some("answer".into()),
            approval_id,
            approved,
            note: None,
            cache: Default::default(),
        })
        .await;
        let done = responses(&mut peer, &["remember", "answer"]).await;
        assert_eq!(done["remember"], approved);
        assert!(done["answer"]);
        assert_eq!(record.exists(), approved);
        peer.send(StdioRequest::NewSession {
            id: Some("idle".into()),
        })
        .await;
        let second = peer.identity().await;
        assert!(peer.response("idle").await.0);
        assert_ne!(first.session_id, second.session_id);
        peer.child.kill().await.unwrap();
    }
}

#[tokio::test]
async fn direct_remember_is_cancellable_by_its_request_id() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let (config, record) = memory_config(home.path());
    let mut peer = Peer::launch(&config, workspace.path());
    peer.identity().await;
    remember(&mut peer).await;
    approval(&mut peer).await;
    peer.send(StdioRequest::Cancel {
        id: Some("cancel".into()),
        target_id: "remember".into(),
    })
    .await;
    let done = responses(&mut peer, &["remember", "cancel"]).await;
    assert!(done["cancel"]);
    assert!(!done["remember"]);
    assert!(!record.exists());
    peer.child.kill().await.unwrap();
}

#[tokio::test]
async fn shutdown_cancels_pending_remember_and_joins_its_response() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let (config, record) = memory_config(home.path());
    let mut peer = Peer::launch(&config, workspace.path());
    peer.identity().await;
    remember(&mut peer).await;
    approval(&mut peer).await;
    peer.send(StdioRequest::Shutdown {
        id: Some("shutdown".into()),
    })
    .await;
    let mut done = HashMap::new();
    while done.len() < 2 {
        let line = tokio::time::timeout(Duration::from_secs(20), peer.output.next_line())
            .await
            .expect("shutdown remains responsive")
            .unwrap()
            .expect("responses precede EOF");
        if let StdioOutput::Response {
            id: Some(id), ok, ..
        } = serde_json::from_str(&line).unwrap()
            && matches!(id.as_str(), "remember" | "shutdown")
        {
            done.insert(id, ok);
        }
    }
    assert!(!done["remember"]);
    assert!(done["shutdown"]);
    assert!(!record.exists());
    assert!(
        tokio::time::timeout(Duration::from_secs(20), peer.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}
