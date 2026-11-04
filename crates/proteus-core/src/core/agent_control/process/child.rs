//! Lifecycle дочернего процесса `proteus server stdio`: spawn с piped
//! stdio, ограниченный reader stdout, запись JSONL-запросов
//! в stdin, kill по требованию.
//!
//! Reader-таск декаплит чтение от turn-логики: пока родитель ждёт approval
//! у пользователя, события ребёнка буферизуются до явного лимита. stderr ребёнка
//! уходит в null — диагностика ребёнка живёт в его собственном event log.

use std::{path::Path, process::Stdio};

use anyhow::{Context, Result, anyhow};
use proteus_contracts::app_protocol::{
    AppServerEvent, AppSubagentSession, StdioOutput, StdioRequest,
};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, ChildStdin, Command},
};

use super::output::{ChildOutputs, OutputHealth};
use crate::contracts::CancellationToken;
use tokio::time::Instant;

pub(super) struct ChildProcess {
    child: Child,
    stdin: ChildStdin,
    outputs: ChildOutputs,
    deadline: Option<Instant>,
    cancellation: Option<CancellationToken>,
    session: Option<AppSubagentSession>,
}

impl ChildProcess {
    pub fn spawn(
        binary: &Path,
        config_ref: &str,
        extra_args: &[String],
        cwd: &Path,
        parent_session_id: crate::domain::SessionId,
        depth_budget: u64,
    ) -> Result<Self> {
        let mut command = Command::new(binary);
        command
            .arg("--config")
            .arg(config_ref)
            .arg("--cwd")
            .arg(cwd)
            .arg("--new-session")
            .arg("--parent-session-id")
            .arg(parent_session_id.to_string())
            .args(extra_args)
            .arg("server")
            .arg("stdio")
            .env(super::config::DEPTH_BUDGET_ENV, depth_budget.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to spawn subagent child process {} (config {config_ref})",
                binary.display()
            )
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("subagent child stdin is not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("subagent child stdout is not piped"))?;

        let outputs = ChildOutputs::spawn(stdout);

        Ok(Self {
            child,
            stdin,
            outputs,
            deadline: None,
            cancellation: None,
            session: None,
        })
    }

    pub async fn send(&mut self, request: &StdioRequest) -> Result<()> {
        self.outputs.health().check()?;
        let mut line = serde_json::to_string(request).context("serialize child stdio request")?;
        line.push('\n');
        let health = self.outputs.health();
        let deadline = self.deadline;
        let cancellation = self.cancellation.clone();
        let result = tokio::select! {
            biased;
            _ = async {
                match cancellation { Some(token) => token.cancelled().await, None => std::future::pending().await }
            } => Err(anyhow!("subagent stdin write cancelled")),
            _ = async {
                match deadline { Some(deadline) => tokio::time::sleep_until(deadline).await, None => std::future::pending().await }
            } => Err(anyhow!("subagent stdin write deadline exceeded")),
            error = health.stopped() => Err(error),
            result = async {
                self.stdin.write_all(line.as_bytes()).await.context("write to subagent child stdin")?;
                self.stdin.flush().await.context("flush subagent child stdin")
            } => result,
        };
        // A failed or interrupted write may leave a partial JSONL frame.
        // Such a connection must never be returned to the process pool.
        if result.is_err() {
            self.kill().await;
        }
        result
    }

    pub(super) fn set_write_budget(
        &mut self,
        deadline: Option<Instant>,
        cancellation: Option<CancellationToken>,
    ) {
        self.deadline = deadline;
        self.cancellation = cancellation;
    }

    /// Следующий output ребёнка. `None` — stdout закрыт (ребёнок умер).
    pub async fn next_output(&mut self) -> Result<Option<StdioOutput>> {
        let output = self.outputs.next().await?;
        if let Some(output) = &output {
            observe_session(&mut self.session, output);
        }
        Ok(output)
    }

    pub fn output_health(&self) -> OutputHealth {
        self.outputs.health()
    }

    /// Выгребает накопившиеся с прошлого turn-а outputs, не блокируясь.
    pub fn drain_stale_outputs(&mut self) -> Result<()> {
        self.outputs
            .drain(|output| observe_session(&mut self.session, output))
    }

    pub(super) fn session(&self) -> Option<&AppSubagentSession> {
        self.session.as_ref()
    }

    /// Bootstrap and rotation obey the same deadline/cancellation as Send.
    async fn next_control_output(&mut self) -> Result<StdioOutput> {
        let cancellation = self.cancellation.clone();
        let deadline = self.deadline;
        tokio::select! {
            biased;
            _ = async { match cancellation { Some(token) => token.cancelled().await, None => std::future::pending().await } } => anyhow::bail!("subagent session setup cancelled"),
            _ = async { match deadline { Some(deadline) => tokio::time::sleep_until(deadline).await, None => std::future::pending().await } } => anyhow::bail!("subagent session setup deadline exceeded"),
            output = self.next_output() => output?.ok_or_else(|| anyhow!("subagent child exited during session setup")),
        }
    }

    pub(super) async fn ensure_session(&mut self) -> Result<AppSubagentSession> {
        while self.session.is_none() {
            match self.next_control_output().await? {
                StdioOutput::Event { event } if matches!(*event, AppServerEvent::Shutdown) => {
                    anyhow::bail!("subagent child shut down during session setup")
                }
                _ => {}
            }
        }
        Ok(self.session.clone().expect("session checked"))
    }

    pub(super) async fn new_session(&mut self) -> Result<AppSubagentSession> {
        let request_id = crate::domain::new_call_id();
        let previous = self.session.take();
        self.send(&StdioRequest::NewSession {
            id: Some(request_id.clone()),
        })
        .await?;
        loop {
            match self.next_control_output().await? {
                StdioOutput::Response {
                    id,
                    ok,
                    output,
                    error,
                } if id.as_deref() == Some(request_id.as_str()) => {
                    anyhow::ensure!(
                        ok,
                        "subagent session rotation failed: {}",
                        error.unwrap_or_else(|| "unknown error".into())
                    );
                    let session: AppSubagentSession = serde_json::from_value(
                        output.context("new session response has no identity")?,
                    )
                    .context("invalid new session identity")?;
                    anyhow::ensure!(
                        self.session.as_ref() == Some(&session),
                        "new session response disagrees with SessionStarted"
                    );
                    anyhow::ensure!(
                        previous
                            .as_ref()
                            .is_none_or(|old| old.session_id != session.session_id),
                        "new session reused the previous identity"
                    );
                    return Ok(session);
                }
                StdioOutput::Event { event } if matches!(*event, AppServerEvent::Shutdown) => {
                    anyhow::bail!("subagent child shut down during session rotation")
                }
                _ => {}
            }
        }
    }

    pub fn is_alive(&mut self) -> bool {
        self.outputs.health().is_running() && matches!(self.child.try_wait(), Ok(None))
    }

    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }

    #[cfg(test)]
    pub(super) fn test_fixture() -> Self {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("while read -r _line; do :; done")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn child fixture");
        let stdin = child.stdin.take().expect("fixture stdin");
        let outputs = ChildOutputs::spawn(child.stdout.take().expect("fixture stdout"));
        Self {
            child,
            stdin,
            outputs,
            deadline: None,
            cancellation: None,
            session: None,
        }
    }
}

fn observe_session(session: &mut Option<AppSubagentSession>, output: &StdioOutput) {
    if let StdioOutput::Event { event } = output
        && let AppServerEvent::Runtime { envelope } = event.as_ref()
        && let crate::domain::Event::SessionStarted {
            session_id,
            session_dir: Some(session_dir),
            ..
        } = &envelope.event
    {
        *session = Some(AppSubagentSession {
            session_id: *session_id,
            session_dir: session_dir.clone(),
        });
    }
}
