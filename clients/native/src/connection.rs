use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use proteus_contracts::app_protocol::AppServerEvent;

use crate::api::{Api, StreamItem};
use crate::backend::Backend;
use crate::launch::{LaunchOptions, Target};

const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);
const RECONNECT_STEP: Duration = Duration::from_millis(500);
/// How often the backend thread checks that the server is still running.
const WATCH_INTERVAL: Duration = Duration::from_millis(500);

/// Owns the spawned app server on a dedicated thread.
///
/// The server is started with a parent-death signal, which Linux ties to the
/// forking thread, so that thread must live as long as the server.
pub struct BackendGuard {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    /// Says why the server exited on its own; closes when it was stopped.
    pub exited: async_channel::Receiver<String>,
}

impl BackendGuard {
    /// Stops the server and waits until it is gone, as the app quits.
    pub fn shutdown(mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for BackendGuard {
    /// Asks the server to stop; its thread terminates it in the background,
    /// so replacing a workspace does not freeze the window.
    fn drop(&mut self) {
        drop(self.stop.take());
    }
}

fn spawn_backend(
    profile: Option<String>,
    workspace: std::path::PathBuf,
) -> Result<(Api, BackendGuard)> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (exited_tx, exited) = async_channel::bounded(1);
    let thread = std::thread::Builder::new()
        .name("proteus-backend".into())
        .spawn(
            move || match Backend::launch(profile.as_deref(), &workspace) {
                Ok(mut backend) => {
                    let api = Api::new(&backend.origin, &backend.token);
                    if ready_tx.send(Ok(api)).is_err() {
                        return;
                    }
                    while let Err(mpsc::RecvTimeoutError::Timeout) =
                        stop_rx.recv_timeout(WATCH_INTERVAL)
                    {
                        if let Some(report) = backend.exit_report() {
                            let _ = exited_tx.try_send(report);
                            break;
                        }
                    }
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            },
        )
        .context("spawn backend thread")?;
    let guard = BackendGuard {
        stop: Some(stop_tx),
        thread: Some(thread),
        exited,
    };
    let api = ready_rx
        .recv()
        .map_err(|_| anyhow!("backend thread exited"))??;
    Ok((api, guard))
}

pub fn connect(options: &LaunchOptions) -> Result<(Api, Option<BackendGuard>)> {
    match &options.target {
        Target::Attach { origin, token } => {
            let origin = proteus_client_common::normalize_local_origin(origin)
                .map_err(|error| anyhow!(error))?;
            Ok((Api::new(&origin, token), None))
        }
        Target::Spawn { profile } => {
            let (api, guard) = spawn_backend(profile.clone(), options.workspace.clone())?;
            Ok((api, Some(guard)))
        }
    }
}

pub enum PumpItem {
    Connected,
    Event(Box<AppServerEvent>),
    Disconnected(String),
}

/// Streams session events on a background thread, reconnecting until the
/// receiver is dropped or the server shuts down. A dropped receiver is
/// noticed on the next event or heartbeat, which ends the thread and closes
/// its connection.
pub fn spawn_pump(api: Api, session_dir: std::path::PathBuf) -> async_channel::Receiver<PumpItem> {
    let (tx, rx) = async_channel::unbounded();
    let spawned = std::thread::Builder::new()
        .name("proteus-events".into())
        .spawn(move || {
            let mut delay = RECONNECT_DELAY;
            loop {
                let error = match api.events(&session_dir) {
                    Ok(mut stream) => {
                        if tx.send_blocking(PumpItem::Connected).is_err() {
                            return;
                        }
                        delay = RECONNECT_DELAY;
                        loop {
                            match stream.next() {
                                Some(Ok(StreamItem::Event(event))) => {
                                    let shutdown = matches!(*event, AppServerEvent::Shutdown);
                                    if tx.send_blocking(PumpItem::Event(event)).is_err() || shutdown
                                    {
                                        return;
                                    }
                                }
                                Some(Ok(StreamItem::Heartbeat)) => {
                                    if tx.is_closed() {
                                        return;
                                    }
                                }
                                Some(Err(error)) => break format!("{error:#}"),
                                None => break "event stream closed".to_owned(),
                            }
                        }
                    }
                    Err(error) => format!("{error:#}"),
                };
                if tx.send_blocking(PumpItem::Disconnected(error)).is_err() {
                    return;
                }
                // Attempts that keep failing come less often; a dropped
                // receiver is still noticed within a step.
                let resume = Instant::now() + delay;
                while Instant::now() < resume {
                    std::thread::sleep(RECONNECT_STEP);
                    if tx.is_closed() {
                        return;
                    }
                }
                delay = (delay * 2).min(MAX_RECONNECT_DELAY);
            }
        });
    if let Err(error) = spawned {
        let (tx, rx) = async_channel::bounded(1);
        let _ = tx.try_send(PumpItem::Disconnected(format!(
            "spawn event thread: {error}"
        )));
        return rx;
    }
    rx
}
