use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex, Weak},
    time::Duration,
};

use tokio::sync::{Mutex, OwnedMutexGuard, broadcast};

use crate::core::AppConfig;

use super::{AppServerEvent, AppServerHandle, AppSessionActivity, security::HttpSecurity};

const WORKER_IDLE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub(super) struct HttpLaunchContext {
    pub(super) config: AppConfig,
    pub(super) cwd: PathBuf,
    pub(super) config_path: Option<PathBuf>,
    pub(super) profile_was_present: bool,
    pub(super) initial_session_dir: Option<PathBuf>,
}

#[derive(Clone)]
pub(super) struct HttpAppState {
    pub(super) launch: Arc<HttpLaunchContext>,
    pub(super) storage: Option<crate::core::StorageService>,
    // Only operations on the same session must wait for terminal settlement.
    // Owners/waiters keep gates alive; the registry keeps only weak references.
    session_lifecycle: Arc<StdMutex<HashMap<PathBuf, Weak<Mutex<()>>>>>,
    pub(super) session_servers: Arc<Mutex<HashMap<PathBuf, AppServerHandle>>>,
    activity_events: broadcast::Sender<SessionActivityUpdate>,
    watched_sessions: Arc<StdMutex<HashSet<PathBuf>>>,
    pub(super) shutdown: broadcast::Sender<()>,
    pub(super) security: HttpSecurity,
}

#[derive(Clone)]
pub(super) struct SessionActivityUpdate {
    pub(super) session_dir: PathBuf,
    pub(super) activity: AppSessionActivity,
}

impl HttpAppState {
    pub(super) fn storage_service(&self) -> anyhow::Result<crate::core::StorageService> {
        self.storage
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no session storage module is selected"))
    }

    #[cfg(test)]
    pub(super) async fn new(
        server: AppServerHandle,
        shutdown: broadcast::Sender<()>,
        security: HttpSecurity,
    ) -> Self {
        let workspace = server.cwd.clone();
        Self::new_for_workspace(server, workspace, shutdown, security).await
    }

    pub(super) async fn new_for_workspace(
        server: AppServerHandle,
        cwd: PathBuf,
        shutdown: broadcast::Sender<()>,
        security: HttpSecurity,
    ) -> Self {
        let launch = HttpLaunchContext {
            config: server.config.read().await.clone(),
            cwd,
            config_path: server.config_path.clone(),
            profile_was_present: server
                .config_path
                .as_ref()
                .is_some_and(|path| path.exists()),
            initial_session_dir: server.session_dir_path(),
        };
        let key = launch.initial_session_dir.clone().map(session_key);
        let mut session_servers = HashMap::new();
        if let Some(key) = &key {
            session_servers.insert(key.clone(), server.clone());
        }
        let (activity_events, _) = broadcast::channel(1024);
        let state = Self {
            launch: Arc::new(launch),
            storage: server.storage.clone(),
            session_lifecycle: Arc::default(),
            session_servers: Arc::new(Mutex::new(session_servers)),
            activity_events,
            watched_sessions: Arc::new(StdMutex::new(HashSet::new())),
            shutdown,
            security,
        };
        if let Some(key) = key {
            state.watch_server(&server, key);
        }
        state
    }

    pub(super) fn subscribe_activity(&self) -> broadcast::Receiver<SessionActivityUpdate> {
        self.activity_events.subscribe()
    }

    /// `session_dir` is the canonical identity, including unmaterialized paths.
    /// A delete keeps this gate through settlement and storage removal, so a
    /// resume cannot reopen its retiring generation. Other sessions stay usable.
    pub(super) async fn lock_session_lifecycle(&self, session_dir: &Path) -> OwnedMutexGuard<()> {
        let gate = {
            let mut gates = self
                .session_lifecycle
                .lock()
                .expect("session lifecycle lock");
            gates.retain(|_, gate| gate.strong_count() > 0);
            match gates.get(session_dir).and_then(Weak::upgrade) {
                Some(gate) => gate,
                None => {
                    let gate = Arc::new(Mutex::new(()));
                    gates.insert(session_dir.to_path_buf(), Arc::downgrade(&gate));
                    gate
                }
            }
        };
        gate.lock_owned().await
    }

    pub(super) async fn remember_server(&self, server: AppServerHandle) {
        if let Some(session_dir) = server.session_dir_path() {
            let key = session_key(session_dir.clone());
            self.session_servers
                .lock()
                .await
                .insert(key.clone(), server.clone());
            self.watch_server(&server, key.clone());
            self.emit_session_activity_for_dir(&key).await;
        }
    }

    /// Resolve caller spelling through live descriptors first. This also works
    /// before an admitted session has materialized its storage directory.
    pub(super) async fn resolve_session_dir(&self, path: PathBuf) -> anyhow::Result<PathBuf> {
        let key = session_key(path.clone());
        {
            let servers = self.session_servers.lock().await;
            for (dir, server) in servers.iter() {
                if *dir == key
                    || server
                        .runtime
                        .session_store()
                        .is_some_and(|store| session_key(store.journal_path()) == key)
                {
                    return Ok(dir.clone());
                }
            }
        }
        let Some(storage) = self.storage.clone() else {
            return Ok(key);
        };
        let opened =
            tokio::task::spawn_blocking(move || crate::core::SessionStore::open(storage, path))
                .await?;
        // Missing/deleted paths still need their lifecycle gate before resume
        // or delete determines the authoritative outcome.
        Ok(opened
            .map(|store| session_key(store.session_dir().to_owned()))
            .unwrap_or(key))
    }

    pub(super) async fn remove_session_server(
        &self,
        session_dir: &Path,
    ) -> Option<AppServerHandle> {
        let key = session_key(session_dir.to_path_buf());
        self.watched_sessions
            .lock()
            .expect("session watcher lock")
            .remove(&key);
        self.session_servers.lock().await.remove(&key)
    }

    pub(super) async fn server_for_session_dir(
        &self,
        session_dir: &Path,
    ) -> Option<AppServerHandle> {
        let key = session_key(session_dir.to_path_buf());
        self.session_servers.lock().await.get(&key).cloned()
    }

    pub(super) async fn all_servers(&self) -> Vec<AppServerHandle> {
        self.session_servers
            .lock()
            .await
            .values()
            .cloned()
            .collect()
    }

    pub(super) async fn activity_for_server(&self, server: &AppServerHandle) -> AppSessionActivity {
        server
            .session_activity(server.running_run_ids().await)
            .await
    }

    pub(super) async fn activity_by_session_dir(&self) -> HashMap<PathBuf, AppSessionActivity> {
        let mut activity = HashMap::new();
        for server in self.all_servers().await {
            if let Some(session_dir) = server.session_dir_path() {
                activity.insert(
                    session_key(session_dir),
                    self.activity_for_server(&server).await,
                );
            }
        }
        activity
    }

    pub(super) async fn emit_session_activity_for_dir(&self, session_dir: &Path) {
        let Some(server) = self.server_for_session_dir(session_dir).await else {
            return;
        };
        publish_activity(
            &self.activity_events,
            &server,
            session_key(session_dir.to_path_buf()),
        )
        .await;
    }

    pub(super) async fn emit_session_activity_for_server(&self, server: &AppServerHandle) {
        if let Some(session_dir) = server.session_dir_path() {
            self.emit_session_activity_for_dir(&session_dir).await;
        }
    }

    // The watcher follows the registered key, not a new resolution of the
    // caller's locator. Activity and retirement use this same registry entry.
    fn watch_server(&self, server: &AppServerHandle, session_dir: PathBuf) {
        {
            let mut watched = self
                .watched_sessions
                .lock()
                .expect("watched session lock poisoned");
            if !watched.insert(session_dir.clone()) {
                return;
            }
        }

        let registry = Arc::downgrade(&self.session_servers);
        let watched = Arc::downgrade(&server.inner);
        let activity = self.activity_events.clone();
        let mut shutdown = self.shutdown.subscribe();
        let mut events = server.events.subscribe_events();
        tokio::spawn(async move {
            let registered_server = async || {
                let registry = registry.upgrade()?;
                let servers = registry.lock().await;
                servers
                    .get(&session_dir)
                    .filter(|server| Arc::as_ptr(&server.inner) == watched.as_ptr())
                    .cloned()
            };
            let Some(server) = registered_server().await else {
                return;
            };
            let mut idle = server.watch_worker_idle().await;
            drop(server);
            let mut reclaim_since = *idle.borrow_and_update();
            loop {
                let wait_until_idle = async move {
                    match reclaim_since {
                        Some(since) => tokio::time::sleep_until(since + WORKER_IDLE_TIMEOUT).await,
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    biased;
                    _ = shutdown.recv() => break,
                    changed = idle.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        reclaim_since = *idle.borrow_and_update();
                    }
                    _ = wait_until_idle => {
                        let since = reclaim_since.take();
                        let Some(server) = registered_server().await else {
                            break;
                        };
                        if let Err(error) = server.release_idle_component_workers_since(since).await {
                            eprintln!("warning: could not release idle component workers: {error:#}");
                        }
                    }
                    received = events.recv() => {
                        let (update, stop) = match received {
                            Ok(event) => (
                                app_event_affects_session_activity(&event.event),
                                matches!(&*event.event, AppServerEvent::Shutdown),
                            ),
                            Err(broadcast::error::RecvError::Lagged(_)) => (true, false),
                            Err(broadcast::error::RecvError::Closed) => break,
                        };
                        if update {
                            let Some(server) = registered_server().await else {
                                break;
                            };
                            publish_activity(&activity, &server, session_dir.clone()).await;
                        }
                        if stop {
                            break;
                        }
                    }
                }
            }
        });
    }
}

async fn publish_activity(
    activity_events: &broadcast::Sender<SessionActivityUpdate>,
    server: &AppServerHandle,
    session_dir: PathBuf,
) {
    let activity = server
        .session_activity(server.running_run_ids().await)
        .await;
    let _ = activity_events.send(SessionActivityUpdate {
        session_dir,
        activity,
    });
}

fn app_event_affects_session_activity(event: &AppServerEvent) -> bool {
    matches!(
        event,
        AppServerEvent::UserMessageSubmitted { .. }
            | AppServerEvent::ExecutionUpdated { .. }
            | AppServerEvent::TurnOutput { .. }
            | AppServerEvent::ApprovalRequested { .. }
            | AppServerEvent::ApprovalResolved { .. }
            | AppServerEvent::UserInputRequested { .. }
            | AppServerEvent::UserInputResolved { .. }
            | AppServerEvent::Error { .. }
            | AppServerEvent::Shutdown
    )
}

pub(super) fn session_key(session_dir: PathBuf) -> PathBuf {
    crate::core::canonicalize_session_dir_path(session_dir.clone()).unwrap_or(session_dir)
}
