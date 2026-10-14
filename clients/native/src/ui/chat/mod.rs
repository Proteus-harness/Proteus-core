//! One open session as a self-contained view: the thread, pending requests,
//! notices and the composer. Every chat tab owns a [`Chat`]; it streams its
//! session's events while it lives and tells the workspace what matters
//! outside through [`ChatEvent`].

mod actions;
mod attachments;
mod composer;
pub(super) mod context_usage;
pub(super) mod controls;
mod images;
mod notice;
mod pump;
mod replies;
mod requests;
mod scroller;
mod session;
mod thread;
mod tool_chain;
pub(super) mod tool_row;
mod user_input;
mod view;

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::*;
use proteus_contracts::app_protocol::AppSessionActivity;

pub(super) use session::{ChatState, Session};

use super::motion::Recent;
use super::prefs::{self, ClientPreferences};
use crate::api::Api;
use crate::preferences::{SendMode, ViewPreferences};
use attachments::Attachments;
use images::SentImages;
use notice::Notice;
use replies::Replies;
use scroller::ChatScroller;
use user_input::UserInputDrafts;

/// Content the host adds to the composer row, e.g. extension icons: the
/// left end (`false`) or the right one next to sending (`true`). It gets the
/// chat's entity id, so the host can show it in one chat only.
pub(super) type ComposerExtras = Rc<dyn Fn(EntityId, bool, &App) -> Option<AnyElement>>;

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
    scroller: Entity<ChatScroller>,
    composer: Entity<TextareaState>,
    attachments: Attachments,
    sent_images: SentImages,
    replies: Replies,
    expanded: HashSet<u64>,
    /// Folded tool runs the user opened, by the key of their first row.
    open_chains: HashSet<u64>,
    /// Rows pushed by live events, for enter motion.
    fresh_rows: Recent,
    /// Approval and question cards that just arrived, for enter motion.
    fresh_requests: Recent,
    user_input: UserInputDrafts,
    notice: Notice,
    extras: ComposerExtras,
    /// Sizes the row heights were measured with.
    view: ViewPreferences,
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
        let autoscroll = prefs::current(cx).view.autoscroll;
        let scroller = cx.new(|cx| ChatScroller::new(autoscroll, cx));
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
                // Row heights are cached; sizes change with them.
                let view = prefs::current(cx).view;
                if this.view != view {
                    this.view = view;
                    this.scroller.update(cx, |scroller, cx| {
                        scroller.set_autoscroll(view.autoscroll, cx);
                        scroller.remeasure(cx);
                    });
                }
            }),
        ];
        let pump = pump::start(api.clone(), session_dir.clone(), cx);
        let mut chat = Self {
            api,
            session: Session::new(session_dir),
            scroller,
            composer,
            attachments: Attachments::default(),
            sent_images: SentImages::default(),
            replies: Replies::default(),
            expanded: HashSet::new(),
            open_chains: HashSet::new(),
            fresh_rows: Recent::default(),
            fresh_requests: Recent::default(),
            user_input: UserInputDrafts::default(),
            notice: Notice::default(),
            extras,
            view: prefs::current(cx).view,
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
                    Err(error) => this.show_notice(format!("{error:#}"), cx),
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
