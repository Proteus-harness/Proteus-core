use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use serde::Deserialize;

const READY_TIMEOUT: Duration = Duration::from_secs(90);
const SETTLE_TIMEOUT: Duration = Duration::from_secs(2);
const LOG_LINES: usize = 60;

#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum ReadyRecord {
    #[serde(rename = "http_ready")]
    HttpReady { origin: String },
}

/// The `proteus server http` process owned by this client.
pub struct Backend {
    child: Child,
    pub origin: String,
    pub token: String,
    log: Arc<Mutex<VecDeque<String>>>,
    /// The stderr reader, done once the server's last words are logged.
    stderr: JoinHandle<()>,
}

impl Backend {
    pub fn launch(profile: Option<&str>, workspace: &Path) -> Result<Self> {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let binary = proteus_binary();
        let mut command = Command::new(&binary);
        // Modules are resolved next to the server binary first, as in the
        // portable bundle.
        if let Some(dir) = binary.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            let inherited = std::env::var_os("PATH").unwrap_or_default();
            let paths = std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&inherited));
            command.env("PATH", std::env::join_paths(paths)?);
        }
        if let Some(profile) = profile {
            command.arg("--config").arg(profile);
        }
        command
            .current_dir(workspace)
            .arg("--cwd")
            .arg(workspace)
            .args([
                "server",
                "http",
                "--port",
                "0",
                "--token",
                &token,
                "--ready-stdout",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        // SAFETY: prctl is async-signal-safe; the server must not outlive
        // this client even when it is killed by a signal.
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().context("failed to start `proteus`")?;

        let log = Arc::new(Mutex::new(VecDeque::new()));
        let (ready_tx, ready_rx) = mpsc::channel();
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        spawn_reader(stdout, Arc::clone(&log), token.clone(), Some(ready_tx));
        let stderr = spawn_reader(stderr, Arc::clone(&log), token.clone(), None);

        let mut backend = Self {
            child,
            origin: String::new(),
            token,
            log,
            stderr,
        };
        match ready_rx.recv_timeout(READY_TIMEOUT) {
            Ok(origin) => {
                backend.origin = proteus_client_common::normalize_local_origin(&origin)
                    .map_err(|error| anyhow!("server reported invalid origin: {error}"))?;
                Ok(backend)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Err(anyhow!(
                "server did not become ready in {} s\n{}",
                READY_TIMEOUT.as_secs(),
                backend.diagnostics()
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let ending = match backend.settle() {
                    Some(status) => format!("server exited before it was ready ({status})"),
                    None => "server closed its output before it was ready".to_owned(),
                };
                Err(anyhow!("{ending}\n{}", backend.diagnostics()))
            }
        }
    }

    pub fn diagnostics(&self) -> String {
        let log = self.log.lock().expect("log lock");
        log.iter().cloned().collect::<Vec<_>>().join("\n")
    }

    /// Why the server ended, once it has exited on its own.
    pub fn exit_report(&mut self) -> Option<String> {
        self.child.try_wait().ok().flatten()?;
        let status = self.settle()?;
        Some(format!("server exited ({status})\n{}", self.diagnostics()))
    }

    /// Waits briefly for a server that closed its stdout to exit and for its
    /// last stderr lines, which explain why, to reach the log.
    fn settle(&mut self) -> Option<ExitStatus> {
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        let mut status = None;
        while Instant::now() < deadline {
            if status.is_none() {
                status = self.child.try_wait().ok().flatten();
            }
            if status.is_some() && self.stderr.is_finished() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        status
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        let group = self.child.id() as libc::pid_t;
        // SAFETY: the server was started as the leader of its own group.
        unsafe { libc::killpg(group, libc::SIGTERM) };
        for _ in 0..30 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        unsafe { libc::killpg(group, libc::SIGKILL) };
        let _ = self.child.wait();
    }
}

fn proteus_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("PROTEUS_BIN") {
        return path.into();
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("proteus")))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("proteus"))
}

fn spawn_reader(
    stream: impl Read + Send + 'static,
    log: Arc<Mutex<VecDeque<String>>>,
    token: String,
    mut ready: Option<mpsc::Sender<String>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        // A line that is not UTF-8 must not stop the reader: the pipe would
        // fill up and block the server on its next log write.
        let mut stream = BufReader::new(stream);
        let mut bytes = Vec::new();
        loop {
            bytes.clear();
            match stream.read_until(b'\n', &mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let line = String::from_utf8_lossy(&bytes);
            let line = line.trim_end_matches(['\n', '\r']);
            if let Some(send) = ready.as_ref()
                && let Ok(ReadyRecord::HttpReady { origin }) = serde_json::from_str(line)
            {
                let _ = send.send(origin);
                ready = None;
                continue;
            }
            let mut log = log.lock().expect("log lock");
            if log.len() == LOG_LINES {
                log.pop_front();
            }
            log.push_back(line.replace(&token, "<token>"));
        }
    })
}
