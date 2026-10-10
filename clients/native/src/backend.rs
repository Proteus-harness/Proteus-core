use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use serde::Deserialize;

const READY_TIMEOUT: Duration = Duration::from_secs(90);
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
}

impl Backend {
    pub fn launch(profile: &str, workspace: &Path) -> Result<Self> {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let binary = proteus_binary();
        let mut command = Command::new(&binary);
        // Modules are resolved next to the server binary first, as in the
        // desktop bundle.
        if let Some(dir) = binary.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            let inherited = std::env::var_os("PATH").unwrap_or_default();
            let paths = std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&inherited));
            command.env("PATH", std::env::join_paths(paths)?);
        }
        command
            .current_dir(workspace)
            .arg("--config")
            .arg(profile)
            .arg("--cwd")
            .arg(workspace)
            .args(["server", "http", "--port", "0", "--token", &token, "--ready-stdout"])
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
        spawn_reader(stderr, Arc::clone(&log), token.clone(), None);

        let mut backend = Self {
            child,
            origin: String::new(),
            token,
            log,
        };
        match ready_rx.recv_timeout(READY_TIMEOUT) {
            Ok(origin) => {
                backend.origin = proteus_app_common::normalize_local_origin(&origin)
                    .map_err(|error| anyhow!("server reported invalid origin: {error}"))?;
                Ok(backend)
            }
            Err(_) => Err(anyhow!(
                "server did not become ready\n{}",
                backend.diagnostics()
            )),
        }
    }

    pub fn diagnostics(&self) -> String {
        let log = self.log.lock().expect("log lock");
        log.iter().cloned().collect::<Vec<_>>().join("\n")
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
) {
    thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            if let Some(send) = ready.as_ref()
                && let Ok(ReadyRecord::HttpReady { origin }) = serde_json::from_str(&line)
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
    });
}
