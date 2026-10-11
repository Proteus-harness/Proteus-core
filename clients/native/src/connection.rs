use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use proteus_contracts::app_protocol::AppServerEvent;

use crate::api::{Api, StreamItem};
use crate::backend::Backend;
use crate::launch::{LaunchOptions, Target};

const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// Owns the spawned app server on a dedicated thread.
///
/// The server is started with a parent-death signal, which Linux ties to the
/// forking thread, so that thread must live as long as the server.
pub struct BackendGuard {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for BackendGuard {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn spawn_backend(profile: String, workspace: std::path::PathBuf) -> Result<(Api, BackendGuard)> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let thread = std::thread::Builder::new()
        .name("proteus-backend".into())
        .spawn(move || match Backend::launch(&profile, &workspace) {
            Ok(backend) => {
                let api = Api::new(&backend.origin, &backend.token);
                if ready_tx.send(Ok(api)).is_ok() {
                    let _ = stop_rx.recv();
                }
                drop(backend);
            }
            Err(error) => {
                let _ = ready_tx.send(Err(error));
            }
        })
        .context("spawn backend thread")?;
    let guard = BackendGuard {
        stop: Some(stop_tx),
        thread: Some(thread),
    };
    let api = ready_rx
        .recv()
        .map_err(|_| anyhow!("backend thread exited"))??;
    Ok((api, guard))
}

pub fn connect(options: &LaunchOptions) -> Result<(Api, Option<BackendGuard>)> {
    match &options.target {
        Target::Attach { origin, token } => {
            let origin = proteus_app_common::normalize_local_origin(origin)
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
            loop {
                let error = match api.events(&session_dir) {
                    Ok(mut stream) => {
                        if tx.send_blocking(PumpItem::Connected).is_err() {
                            return;
                        }
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
                std::thread::sleep(RECONNECT_DELAY);
                if tx.is_closed() {
                    return;
                }
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
