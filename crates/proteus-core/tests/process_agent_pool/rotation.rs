//! Real stdio connection: idle rotation keeps the OS process and retires its
//! event bridge. Command admission cannot be bypassed by NewSession.
use super::*;
use proteus_contracts::app_protocol::{
    AppServerEvent, AppSubagentSession, StdioOutput, StdioRequest,
};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
};

struct Peer {
    child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
    active: bool,
}

impl Peer {
    fn launch(config: &std::path::Path, cwd: &std::path::Path) -> Self {
        Self::launch_selected(config, cwd, &["--new-session"])
    }

    fn launch_selected(
        config: &std::path::Path,
        cwd: &std::path::Path,
        selection: &[&str],
    ) -> Self {
        let mut child = Command::new(proteus_binary())
            .args([
                "--config",
                config.to_str().unwrap(),
                "--cwd",
                cwd.to_str().unwrap(),
            ])
            .args(selection)
            .args(["server", "stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap()).lines();
        Self {
            child,
            input,
            output,
            active: false,
        }
    }

    async fn send(&mut self, request: StdioRequest) {
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        self.input.write_all(&bytes).await.unwrap();
        self.input.flush().await.unwrap();
    }

    async fn next(&mut self) -> StdioOutput {
        let line = tokio::time::timeout(Duration::from_secs(20), self.output.next_line())
            .await
            .expect("bounded stdio output")
            .unwrap()
            .expect("stdio still open");
        let output: StdioOutput = serde_json::from_str(&line).unwrap();
        if let StdioOutput::Event { event } = &output
            && let AppServerEvent::ExecutionUpdated { execution } = event.as_ref()
        {
            self.active = execution.active.is_some();
        }
        assert!(
            !matches!(&output, StdioOutput::Event { event }
            if matches!(event.as_ref(), AppServerEvent::Shutdown)),
            "retired bridge leaked Shutdown"
        );
        output
    }

    async fn identity(&mut self) -> AppSubagentSession {
        loop {
            if let StdioOutput::Event { event } = self.next().await
                && let AppServerEvent::Runtime { envelope } = *event
                && let Event::SessionStarted {
                    session_id,
                    session_dir: Some(session_dir),
                    ..
                } = envelope.event
            {
                return AppSubagentSession {
                    session_id,
                    session_dir,
                };
            }
        }
    }

    async fn response(&mut self, request: &str) -> (bool, Option<serde_json::Value>) {
        loop {
            if let StdioOutput::Response { id, ok, output, .. } = self.next().await
                && id.as_deref() == Some(request)
            {
                return (ok, output);
            }
        }
    }
}

#[tokio::test]
async fn populated_explicit_and_default_resumes_do_not_wait_for_a_new_startup_event() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let config = write_child_config(home.path());
    let mut first = Peer::launch(&config, workspace.path());
    let identity = first.identity().await;
    first
        .send(StdioRequest::Send {
            id: Some("save".into()),
            text: "saved turn".into(),
            images: vec![],
            options: Default::default(),
        })
        .await;
    assert!(first.response("save").await.0);
    first.child.kill().await.unwrap();
    for explicit in [true, false] {
        let selection = if explicit {
            vec!["--resume-session", identity.session_dir.to_str().unwrap()]
        } else {
            vec![]
        };
        let mut resumed = Peer::launch_selected(&config, workspace.path(), &selection);
        loop {
            if let StdioOutput::Event { event } = resumed.next().await
                && let AppServerEvent::SessionSnapshot { snapshot } = *event
            {
                assert_eq!(snapshot.session_id, identity.session_id);
                break;
            }
        }
        resumed
            .send(StdioRequest::HistorySummary {
                id: Some("history".into()),
            })
            .await;
        let (ok, history) = resumed.response("history").await;
        assert!(ok);
        assert!(history.unwrap()["messages"].as_u64().unwrap() > 0);
        resumed.child.kill().await.unwrap();
    }
}

#[tokio::test]
async fn idle_rotation_keeps_process_and_new_bridge_serves_next_turn() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let config = write_child_config(home.path());
    let mut peer = Peer::launch(&config, workspace.path());
    let pid = peer.child.id().unwrap();
    let first = peer.identity().await;
    peer.send(StdioRequest::Send {
        id: Some("first".into()),
        text: "first".into(),
        images: vec![],
        options: Default::default(),
    })
    .await;
    assert!(peer.response("first").await.0);
    let first_journal = std::fs::read(first.session_dir.join("journal.jsonl")).unwrap();
    peer.send(StdioRequest::NewSession {
        id: Some("rotate".into()),
    })
    .await;
    let second = peer.identity().await;
    let (ok, response) = peer.response("rotate").await;
    assert!(ok);
    assert_eq!(response.unwrap(), json!(second));
    assert_ne!(first.session_id, second.session_id);
    assert_ne!(first.session_dir, second.session_dir);
    assert_eq!(peer.child.id(), Some(pid));
    peer.send(StdioRequest::Send {
        id: Some("second".into()),
        text: "second".into(),
        images: vec![],
        options: Default::default(),
    })
    .await;
    assert!(
        peer.response("second").await.0,
        "old Shutdown must not poison the next turn"
    );
    assert_eq!(
        std::fs::read(first.session_dir.join("journal.jsonl")).unwrap(),
        first_journal
    );
    peer.child.kill().await.unwrap();
}

#[tokio::test]
async fn active_module_command_rejects_rotation_then_idle_rotation_succeeds() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let config = write_child_config(home.path());
    let worker =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/process_commands.py");
    let mut config_text = std::fs::read_to_string(&config).unwrap();
    config_text.push_str(&format!(
        r#"
[modules]
policy = "probe.policy"
[tools]
enabled = ["probe_command"]
[components.command-probe]
command = "python3"
args = [{}]
[components.command-probe.exports.tool."probe.commands"]
[components.command-probe.exports.policy."probe.policy"]
"#,
        serde_json::to_string(&worker).unwrap()
    ));
    std::fs::write(&config, config_text).unwrap();
    let mut peer = Peer::launch(&config, workspace.path());
    let first = peer.identity().await;
    peer.send(StdioRequest::ExecuteCommand {
        id: Some("command".into()),
        text: "/probe wait".into(),
    })
    .await;
    loop {
        if let StdioOutput::Event { event } = peer.next().await
            && matches!(*event, AppServerEvent::ApprovalRequested { .. })
        {
            break;
        }
    }
    peer.send(StdioRequest::NewSession {
        id: Some("busy".into()),
    })
    .await;
    assert!(!peer.response("busy").await.0);
    peer.send(StdioRequest::Cancel {
        id: Some("cancel".into()),
        target_id: "command".into(),
    })
    .await;
    // The command response may race the cancel response; its settled execution
    // update is already published before either command terminal is emitted.
    assert!(peer.response("cancel").await.0);
    while peer.active {
        peer.next().await;
    }
    peer.send(StdioRequest::NewSession {
        id: Some("idle".into()),
    })
    .await;
    let second = peer.identity().await;
    assert!(peer.response("idle").await.0);
    assert_ne!(first.session_id, second.session_id);
    peer.child.kill().await.unwrap();
}
