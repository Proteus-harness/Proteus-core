use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use anyhow::Result;
use gpui_kit::*;
use proteus_app_common::session_selection::select_startup_session;
use proteus_contracts::app_protocol::AppSessionSummary;

use super::chats::OpenChat;
use super::extensions::{OpenDocument, SurfaceView};
use super::panels::Renaming;
use super::settings::SettingsView;
use super::tabs::{SplitResize, TabDrop, TabMotion};
use crate::api::Api;
use crate::connection::{self, BackendGuard};
use crate::launch::LaunchOptions;
use crate::tabs::{self, PaneId};

pub(super) enum Status {
    Starting,
    Ready,
    Failed(SharedString),
}

struct Boot {
    api: Api,
    backend: Option<BackendGuard>,
    sessions: Vec<AppSessionSummary>,
    /// Chats of the saved layout whose sessions still exist, resumed.
    restored: Vec<PathBuf>,
    /// Session to open when no chat was restored.
    startup: Option<PathBuf>,
}

fn boot(options: &LaunchOptions, saved: Vec<PathBuf>) -> Result<Boot> {
    let (api, backend) = connection::connect(options)?;
    let mut sessions = api.sessions()?;
    let restored: Vec<PathBuf> = saved
        .into_iter()
        .filter(|dir| sessions.iter().any(|summary| &summary.session_dir == dir))
        .filter(|dir| {
            api.resume(dir)
                .is_ok_and(|opened| opened.session_dir == *dir)
        })
        .collect();
    let mut startup = None;
    if restored.is_empty() {
        let bootstrap = api.bootstrap()?;
        startup = Some(
            match select_startup_session(None, bootstrap.session_dir, &sessions) {
                Some(dir) => api.resume(Path::new(&dir))?.session_dir,
                None => {
                    let opened = api.new_session(None)?.session_dir;
                    sessions = api.sessions()?;
                    opened
                }
            },
        );
    }
    Ok(Boot {
        api,
        backend,
        sessions,
        restored,
        startup,
    })
}

pub struct Workspace {
    pub(super) workspace_path: PathBuf,
    pub(super) status: Status,
    api: Option<Api>,
    backend: Option<BackendGuard>,
    pub(super) sessions: Vec<AppSessionSummary>,
    /// When the session list first arrived, so its rows can enter in turn.
    pub(super) sessions_arrived: Option<Instant>,
    /// A session list request is running; `sessions_stale` asks for
    /// another one when it ends.
    pub(super) sessions_loading: bool,
    pub(super) sessions_stale: bool,
    /// Archived sessions are listed.
    pub(super) show_archived: bool,
    /// A session name being edited in the list.
    pub(super) renaming: Option<Renaming>,
    /// Open chat tabs by tab id.
    pub(super) chats: HashMap<String, OpenChat>,
    /// Chats being resumed, and how they open.
    pub(super) opening: HashMap<String, OpenDocument>,
    /// Tab id of the chat that commands and extensions address.
    pub(super) active_chat: Option<String>,
    /// Error of a workspace call while no chat is open.
    pub(super) notice: Option<SharedString>,
    pub(super) window_title: String,
    pub(super) settings: Option<Entity<SettingsView>>,
    pub(super) settings_subscription: Option<Subscription>,
    /// Extension panels, document tabs and icons built so far; the tab
    /// layout itself is a client preference.
    pub(super) panels: HashMap<&'static str, SurfaceView>,
    pub(super) documents: HashMap<String, SurfaceView>,
    pub(super) badges: HashMap<&'static str, AnyView>,
    /// Size of the split whose divider is being dragged.
    pub(super) split_drag: Option<SplitResize>,
    /// A tab is being dragged, so panes show where it can go.
    pub(super) tab_drag: bool,
    /// Where the dragged tab would land when dropped onto a pane.
    pub(super) tab_drop: Option<TabDrop>,
    /// Scroll of each pane's tab strip and the active tab it last showed,
    /// so a newly active tab is scrolled into view.
    pub(super) tab_strips: HashMap<PaneId, (ScrollHandle, Option<String>)>,
    /// Tabs opening and closing, panes moving to new places.
    pub(super) tab_motion: TabMotion,
    /// Size of the tab area at the last paint.
    pub(super) tab_area: Rc<Cell<Option<Size<Pixels>>>>,
    pub(super) focus_handle: FocusHandle,
    _startup: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(options: LaunchOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.on_app_quit(|this, _| {
            let backend = this.backend.take();
            async move { drop(backend) }
        })];

        let workspace_path = options.workspace.clone();
        let saved: Vec<PathBuf> = super::prefs::current(cx)
            .tabs
            .chats()
            .filter_map(tabs::chat_dir)
            .map(Path::to_path_buf)
            .collect();
        let startup = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { boot(&options, saved) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.finish_startup(result, window, cx)
            })
            .ok();
        });

        Self {
            workspace_path,
            status: Status::Starting,
            api: None,
            backend: None,
            sessions: Vec::new(),
            sessions_loading: false,
            sessions_stale: false,
            show_archived: false,
            renaming: None,
            chats: HashMap::new(),
            opening: HashMap::new(),
            active_chat: None,
            notice: None,
            window_title: String::new(),
            settings: None,
            settings_subscription: None,
            panels: HashMap::new(),
            documents: HashMap::new(),
            badges: HashMap::new(),
            split_drag: None,
            tab_drag: false,
            tab_drop: None,
            tab_strips: HashMap::new(),
            sessions_arrived: None,
            tab_motion: TabMotion::default(),
            tab_area: Rc::default(),
            focus_handle: cx.focus_handle(),
            _startup: startup,
            _subscriptions: subscriptions,
        }
    }

    fn finish_startup(
        &mut self,
        result: Result<Boot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(boot) => {
                self.api = Some(boot.api);
                self.backend = boot.backend;
                self.sessions = boot.sessions;
                self.sessions_arrived = Some(Instant::now());
                self.status = Status::Ready;
                self.restore_chats(boot.restored, window, cx);
                if let Some(dir) = boot.startup {
                    self.add_chat(dir, window, cx);
                } else {
                    self.focus_active_chat(window, cx);
                }
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
                    Err(error) => this.report(format!("{error:#}"), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Like [`Self::call`], for results that need the window.
    pub(super) fn call_in<R: Send + 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        job: impl FnOnce(&Api) -> Result<R> + Send + 'static,
        done: impl FnOnce(&mut Self, R, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let Some(api) = self.api.clone() else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { job(&api) })
                .await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(value) => done(this, value, window, cx),
                    Err(error) => this.report(format!("{error:#}"), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
