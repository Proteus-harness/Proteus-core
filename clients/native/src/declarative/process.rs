//! A running extension process: line I/O over stdio.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use anyhow::{Context as _, Result};

use super::manifest::Installed;
use super::protocol::{self, Incoming, Outgoing};

const LOG_LINES: usize = 20;

/// What the reader threads report.
#[derive(Debug)]
pub enum Signal {
    Message(Incoming),
    /// A line that is not a valid message.
    Invalid(String),
    /// The process ended; carries its exit status and last stderr lines.
    Exited(String),
}

pub struct Process {
    child: Child,
    /// Lines go through a writer thread so a process that stops reading
    /// never blocks the UI.
    writer: mpsc::Sender<Vec<u8>>,
}

impl Process {
    pub fn spawn(extension: &Installed) -> Result<(Self, async_channel::Receiver<Signal>)> {
        let (program, args) = extension
            .manifest
            .command
            .split_first()
            .context("command is empty")?;
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(&extension.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        // SAFETY: prctl is async-signal-safe; an extension must not outlive
        // the client.
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("cannot start `{program}`"))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let (tx, rx) = async_channel::unbounded();
        let log = Arc::new(Mutex::new(VecDeque::new()));
        {
            let log = Arc::clone(&log);
            thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let mut log = log.lock().unwrap();
                    if log.len() == LOG_LINES {
                        log.pop_front();
                    }
                    log.push_back(line);
                }
            });
        }
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let signal = match protocol::parse(&line) {
                    Ok(message) => Signal::Message(message),
                    Err(error) => Signal::Invalid(format!("{error:#}")),
                };
                if tx.send_blocking(signal).is_err() {
                    return;
                }
            }
            // stderr may still be draining; give it a moment for the tail.
            thread::sleep(std::time::Duration::from_millis(50));
            let tail: Vec<String> = log.lock().unwrap().iter().cloned().collect();
            let mut text = "процесс расширения завершился".to_string();
            if !tail.is_empty() {
                text.push_str(":\n");
                text.push_str(&tail.join("\n"));
            }
            let _ = tx.send_blocking(Signal::Exited(text));
        });

        let (writer, lines) = mpsc::channel();
        thread::spawn(move || write_lines(stdin, lines));

        Ok((Self { child, writer }, rx))
    }

    /// Writes one message; a dead process shows up as [`Signal::Exited`].
    pub fn send(&self, message: &Outgoing) {
        let Ok(mut line) = serde_json::to_vec(message) else {
            return;
        };
        line.push(b'\n');
        let _ = self.writer.send(line);
    }
}

fn write_lines(mut stdin: ChildStdin, lines: mpsc::Receiver<Vec<u8>>) {
    for line in lines {
        if stdin.write_all(&line).and_then(|()| stdin.flush()).is_err() {
            return;
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::declarative::manifest::{Manifest, Surfaces};
    use crate::declarative::protocol::HostEventKind;

    #[test]
    fn exchanges_lines_and_reports_the_exit() {
        let script = r#"read hello; echo '{"type":"view","view":"panel","root":{"type":"text","text":"ok"}}'; echo 'junk'; echo 'bye' >&2"#;
        let extension = Installed {
            dir: std::env::temp_dir(),
            manifest: Manifest {
                id: "test".into(),
                title: "Test".into(),
                description: String::new(),
                icon: None,
                command: vec!["sh".into(), "-c".into(), script.into()],
                surfaces: Surfaces::default(),
            },
        };
        let (process, signals) = Process::spawn(&extension).unwrap();
        process.send(&Outgoing::Event {
            event: HostEventKind::RunSettled,
            session_dir: None,
        });
        let view = signals.recv_blocking().unwrap();
        assert!(
            matches!(view, Signal::Message(Incoming::View { .. })),
            "{view:?}"
        );
        assert!(matches!(
            signals.recv_blocking().unwrap(),
            Signal::Invalid(_)
        ));
        let Signal::Exited(text) = signals.recv_blocking().unwrap() else {
            panic!("exit expected");
        };
        assert!(text.ends_with("bye"), "{text}");
    }
}
