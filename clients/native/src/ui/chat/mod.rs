//! One open session as a self-contained view: the thread, pending requests,
//! notices and the composer. Every chat tab owns a [`Chat`]; it streams its
//! session's events while it lives and tells the workspace what matters
//! outside through [`ChatEvent`].

mod actions;
mod composer;
pub(super) mod context_usage;
pub(super) mod controls;
mod pump;
mod requests;
mod session;
mod thread;
pub(super) mod tool_row;
mod user_input;
mod view;

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::message_scroller::MessageScrollerState;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppSessionActivity;

pub(super) use session::{ChatState, Session};

use super::motion::Recent;
use super::prefs::{self, ClientPreferences};
use crate::api::Api;
use crate::preferences::SendMode;
use user_input::UserInputDrafts;

/// Content the host adds to the composer row, e.g. extension icons. It gets
/// the chat's entity id, so the host can show it in one chat only.
pub(super) type ComposerExtras = Rc<dyn Fn(EntityId, &App) -> Option<AnyElement>>;

pub(super) enum ChatEvent {
    /// A run of this session finished.
    RunSettled,
    /// Activity of some session; every session stream carries all of them.
    Activity {
        session_dir: PathBuf,
        activity: AppSessionActivity,
    },
    /// The user sent a message, so the chat is worth keeping open.
    Sent,
    /// The app server shut down.
    ServerStopped,
}

pub(super) struct Chat {
    api: Api,
    pub session: Session,
    scroller: Entity<MessageScrollerState>,
    composer: Entity<TextareaState>,
    expanded: HashSet<u64>,
    /// Rows pushed by live events, for enter motion.
    fresh_rows: Recent,
    user_input: UserInputDrafts,
    notice: Option<SharedString>,
    extras: ComposerExtras,
    _pump: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ChatEvent> for Chat {}

impl Chat {
    pub fn new(
        api: Api,
        session_dir: PathBuf,
        extras: ComposerExtras,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let enter_sends = prefs::current(cx).send == SendMode::Enter;
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 12)
                .placeholder("Сообщение для Proteus")
                .submit_on_enter(enter_sends)
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &composer,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    let InputEvent::PressEnter { secondary, shift } = *event else {
                        return;
                    };
                    let sends = match prefs::current(cx).send {
                        SendMode::Enter => !shift,
                        SendMode::CtrlEnter => secondary,
                    };
                    if sends {
                        this.send(window, cx);
                    }
                },
            ),
            cx.observe_global::<ClientPreferences>(|this, cx| {
                let enter_sends = prefs::current(cx).send == SendMode::Enter;
                this.composer
                    .update(cx, |state, cx| state.set_submit_on_enter(enter_sends, cx));
            }),
        ];
        let pump = pump::start(api.clone(), session_dir.clone(), cx);
        let mut chat = Self {
            api,
            session: Session::new(session_dir),
            scroller,
            composer,
            expanded: HashSet::new(),
            fresh_rows: Recent::default(),
            user_input: UserInputDrafts::default(),
            notice: None,
            extras,
            _pump: pump,
            _subscriptions: subscriptions,
        };
        chat.load_config(cx);
        chat.load_context(cx);
        chat
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.composer
            .update(cx, |state, cx| state.focus(window, cx));
    }

    pub fn show_notice(&mut self, notice: String, cx: &mut Context<Self>) {
        self.notice = Some(notice.into());
        cx.notify();
    }

    /// Runs a blocking server call off the UI thread; failures become the
    /// chat's notice.
    fn call<R: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        job: impl FnOnce(&Api) -> anyhow::Result<R> + Send + 'static,
        done: impl FnOnce(&mut Self, R, &mut Context<Self>) + 'static,
    ) {
        let api = self.api.clone();
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
}

impl Focusable for Chat {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.composer.focus_handle(cx)
    }
}
