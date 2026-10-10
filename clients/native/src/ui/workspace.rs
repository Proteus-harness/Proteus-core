use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::message_scroller::MessageScrollerState;
use gpui_kit::*;
use proteus_app_common::pending::PendingCursor;
use proteus_app_common::session_selection::select_startup_session;
use proteus_app_common::sync::SessionCursor;
use proteus_contracts::app_protocol::config::ConfigSummary;
use proteus_contracts::app_protocol::{
    AppExecutionState, AppPendingRequests, AppServerEvent, AppSessionSummary,
};
use proteus_contracts::domain::{Event as RuntimeEvent, ThreadId};

use super::context_usage::ContextUsage;
use super::panel::InfoPanel;
use super::settings::SettingsView;
use super::user_input::UserInputDrafts;
use crate::api::Api;
use crate::connection::{self, BackendGuard, PumpItem};
use crate::launch::LaunchOptions;
use crate::transcript::{Change, Transcript};

pub(super) enum Status {
    Starting,
    Ready,
    Failed(SharedString),
}

pub(super) struct Session {
    pub dir: PathBuf,
    cursor: SessionCursor,
    pending_cursor: PendingCursor,
    pub transcript: Transcript,
    pub execution: AppExecutionState,
    pub pending: Option<AppPendingRequests>,
    pub connected: bool,
    pub link_error: Option<SharedString>,
    pub config: Option<ConfigSummary>,
    pub context: Option<ContextUsage>,
    root_thread: Option<ThreadId>,
}

impl Session {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            cursor: SessionCursor::default(),
            pending_cursor: PendingCursor::default(),
            transcript: Transcript::default(),
            execution: AppExecutionState::default(),
            pending: None,
            connected: false,
            link_error: None,
            config: None,
            context: None,
            root_thread: None,
        }
    }

    pub fn running(&self) -> bool {
        self.execution.active.is_some()
    }
}

struct Boot {
    api: Api,
    backend: Option<BackendGuard>,
    sessions: Vec<AppSessionSummary>,
    session_dir: PathBuf,
}

fn boot(options: &LaunchOptions) -> Result<Boot> {
    let (api, backend) = connection::connect(options)?;
    let mut sessions = api.sessions()?;
    let bootstrap = api.bootstrap()?;
    let session_dir = match select_startup_session(None, bootstrap.session_dir, &sessions) {
        Some(dir) => api.resume(Path::new(&dir))?.session_dir,
        None => {
            let opened = api.new_session(None)?.session_dir;
            sessions = api.sessions()?;
            opened
        }
    };
    Ok(Boot {
        api,
        backend,
        sessions,
        session_dir,
    })
}

pub struct Workspace {
    pub(super) workspace_path: PathBuf,
    pub(super) status: Status,
    api: Option<Api>,
    backend: Option<BackendGuard>,
    pub(super) sessions: Vec<AppSessionSummary>,
    pub(super) session: Option<Session>,
    pub(super) scroller: Entity<MessageScrollerState>,
    pub(super) composer: Entity<TextareaState>,
    pub(super) expanded: HashSet<u64>,
    pub(super) user_input: UserInputDrafts,
    pub(super) notice: Option<SharedString>,
    pub(super) window_title: String,
    pub(super) settings: Option<Entity<SettingsView>>,
    pub(super) settings_subscription: Option<Subscription>,
    pub(super) panel: InfoPanel,
    pump: Option<Task<()>>,
    _startup: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(options: LaunchOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 12)
                .placeholder("Сообщение для Proteus")
                .submit_on_enter(true)
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &composer,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { shift: false, .. } = event {
                        this.send(window, cx);
                    }
                },
            ),
            cx.on_app_quit(|this, _| {
                let backend = this.backend.take();
                async move { drop(backend) }
            }),
        ];
        composer.update(cx, |state, cx| state.focus(window, cx));

        let workspace_path = options.workspace.clone();
        let startup = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { boot(&options) })
                .await;
            this.update(cx, |this, cx| this.finish_startup(result, cx))
                .ok();
        });

        Self {
            workspace_path,
            status: Status::Starting,
            api: None,
            backend: None,
            sessions: Vec::new(),
            session: None,
            scroller,
            composer,
            expanded: HashSet::new(),
            user_input: UserInputDrafts::default(),
            notice: None,
            window_title: String::new(),
            settings: None,
            settings_subscription: None,
            panel: InfoPanel::default(),
            pump: None,
            _startup: startup,
            _subscriptions: subscriptions,
        }
    }

    fn finish_startup(&mut self, result: Result<Boot>, cx: &mut Context<Self>) {
        match result {
            Ok(boot) => {
                self.api = Some(boot.api);
                self.backend = boot.backend;
                self.sessions = boot.sessions;
                self.status = Status::Ready;
                self.open_session(boot.session_dir, cx);
            }
            Err(error) => self.status = Status::Failed(format!("{error:#}").into()),
        }
        cx.notify();
    }

    pub(super) fn api(&self) -> Option<Api> {
        self.api.clone()
    }

    /// Runs a blocking server call off the UI thread.
    pub(super) fn call<R: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        job: impl FnOnce(&Api) -> Result<R> + Send + 'static,
        done: impl FnOnce(&mut Self, R, &mut Context<Self>) + 'static,
    ) {
        let Some(api) = self.api.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { job(&api) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(value) => done(this, value, cx),
                    Err(error) => this.notice = Some(format!("{error:#}").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn open_session(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else {
            return;
        };
        self.session = Some(Session::new(dir.clone()));
        self.expanded.clear();
        self.user_input = UserInputDrafts::default();
        self.scroller
            .update(cx, |scroller, cx| scroller.reset(0, cx));
        self.load_config(cx);
        self.load_context(cx);

        let events = connection::spawn_pump(api, dir);
        self.pump = Some(cx.spawn(async move |this, cx| {
            while let Ok(first) = events.recv().await {
                let mut batch = vec![first];
                while let Ok(item) = events.try_recv() {
                    batch.push(item);
                }
                let applied = this.update(cx, |this, cx| {
                    for item in batch {
                        this.apply(item, cx);
                    }
                    cx.notify();
                });
                if applied.is_err() {
                    break;
                }
            }
        }));
        cx.notify();
    }

    pub(super) fn toggle_expanded(&mut self, key: u64, cx: &mut Context<Self>) {
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
        let row = self.session.as_ref().and_then(|session| {
            session
                .transcript
                .entries()
                .iter()
                .position(|entry| entry.key == key)
        });
        if let Some(row) = row {
            self.scroller.update(cx, |scroller, cx| {
                scroller.remeasure_items(row..row + 1, cx)
            });
        }
        cx.notify();
    }

    fn apply(&mut self, item: PumpItem, cx: &mut Context<Self>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match item {
            PumpItem::Connected => {
                session.cursor.begin_connection();
                session.pending_cursor.begin_connection();
                session.connected = true;
                session.link_error = None;
            }
            PumpItem::Disconnected(error) => {
                session.connected = false;
                session.link_error = Some(error.into());
            }
            PumpItem::Event(event) => self.apply_event(*event, cx),
        }
    }

    fn apply_event(&mut self, event: AppServerEvent, cx: &mut Context<Self>) {
        if let AppServerEvent::SessionActivityUpdated {
            session_dir,
            activity,
        } = &event
        {
            match self
                .sessions
                .iter_mut()
                .find(|summary| &summary.session_dir == session_dir)
            {
                Some(summary) => summary.activity = Some(activity.clone()),
                None => self.refresh_sessions(cx),
            }
            return;
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if !session.cursor.accept(&event) {
            return;
        }
        let change = match event {
            AppServerEvent::SessionSnapshot { snapshot } => {
                let snapshot = *snapshot;
                session.execution = snapshot.execution;
                session.root_thread = snapshot.root_thread_id;
                session.transcript.reset(snapshot.transcript)
            }
            AppServerEvent::ExecutionUpdated { execution } => {
                let settled = session.running() && execution.active.is_none();
                session.execution = execution;
                if settled {
                    self.refresh_sessions(cx);
                    self.load_context(cx);
                    self.load_quota(cx);
                }
                Change::None
            }
            AppServerEvent::Runtime { envelope } => {
                if let RuntimeEvent::TokenUsageUpdated { usage, .. } = &envelope.event
                    && session
                        .root_thread
                        .is_none_or(|root| root == envelope.thread_id)
                {
                    session.context = ContextUsage::from_event(usage);
                }
                session
                    .transcript
                    .apply_runtime(envelope.thread_id, &envelope.event)
            }
            AppServerEvent::UserMessageSubmitted { text, images } => {
                session.transcript.push_user(None, &text, images.len())
            }
            AppServerEvent::PendingRequestsUpdated { snapshot } => {
                if session.pending_cursor.accept_stream(
                    &snapshot.session_id.to_string(),
                    &snapshot.stream_id,
                    snapshot.seq,
                ) {
                    session.pending = Some(*snapshot);
                }
                if let Some(pending) = &session.pending {
                    self.user_input.retain(pending);
                }
                Change::None
            }
            AppServerEvent::ModulesReloaded { .. }
            | AppServerEvent::ProfileReloadStatus { error: None } => {
                self.load_config(cx);
                Change::None
            }
            AppServerEvent::Error { message } => {
                self.notice = Some(message.into());
                Change::None
            }
            AppServerEvent::ProfileReloadStatus { error: Some(error) } => {
                self.notice = Some(format!("Не удалось перезагрузить профиль: {error}").into());
                Change::None
            }
            AppServerEvent::Shutdown => {
                self.status = Status::Failed("Сервер приложения остановлен".into());
                Change::None
            }
            _ => Change::None,
        };
        self.apply_change(change, cx);
    }

    fn apply_change(&mut self, change: Change, cx: &mut Context<Self>) {
        self.scroller.update(cx, |scroller, cx| match change {
            Change::None => {}
            Change::Pushed => {
                scroller.append(1, cx);
            }
            Change::Updated(row) => {
                scroller.remeasure_items(row..row + 1, cx);
            }
            Change::Spliced {
                start,
                removed,
                inserted,
            } => {
                scroller.splice(start..start + removed, inserted, cx);
            }
        });
    }
}
