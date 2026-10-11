//! Open chats of the workspace: one [`Chat`] per chat tab, the active chat
//! that commands and extensions address, and the session list around them.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::*;

use super::chat::{Chat, ChatEvent, ComposerExtras};
use super::extensions::{HostEvent, OpenDocument};
use super::format;
use super::workspace::{Status, Workspace};
use crate::preferences::BadgePlace;
use crate::tabs::{self, TabLayout};

/// A chat view and the subscription to its events.
pub(super) struct OpenChat {
    pub view: Entity<Chat>,
    _events: Subscription,
}

impl Workspace {
    pub(super) fn active_chat_view(&self) -> Option<&Entity<Chat>> {
        self.chats
            .get(self.active_chat.as_ref()?)
            .map(|chat| &chat.view)
    }

    pub(super) fn active_chat_dir(&self) -> Option<&Path> {
        tabs::chat_dir(self.active_chat.as_deref()?)
    }

    /// Title of a session: the name the user gave it, else what the session
    /// list knows.
    pub(super) fn session_title(&self, dir: &Path, cx: &App) -> String {
        if let Some(title) = super::prefs::current(cx).sessions.title(dir) {
            return title.to_owned();
        }
        self.sessions
            .iter()
            .find(|summary| summary.session_dir == dir)
            .map(format::session_title)
            .unwrap_or_else(|| "Новая сессия".to_owned())
    }

    /// Builds the chats of the saved layout and closes chat tabs whose
    /// sessions are gone.
    pub(super) fn restore_chats(
        &mut self,
        dirs: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for dir in dirs {
            self.insert_chat(dir, window, cx);
        }
        let chats: Vec<String> = self.chats.keys().cloned().collect();
        self.update_tabs(cx, |layout| {
            layout.retain(|id| !tabs::is_chat(id) || chats.iter().any(|chat| chat == id))
        });
    }

    /// Opens a resumed or new session as a regular tab with the keyboard in
    /// its composer.
    pub(super) fn add_chat(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.insert_chat(dir, window, cx);
        let open = OpenDocument {
            preview: false,
            focus: true,
        };
        self.show_tab(&id, open, window, cx);
    }

    /// Shows the chat of a session, resuming the session first when its tab
    /// is not open. A preview chat is replaced by the next preview.
    pub(super) fn open_chat(
        &mut self,
        dir: PathBuf,
        open: OpenDocument,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_settings(cx);
        let id = tabs::chat(&dir);
        if self.chats.contains_key(&id) {
            self.show_tab(&id, open, window, cx);
            return;
        }
        // A double click lands while the first click is still resuming.
        if let Some(pending) = self.opening.get_mut(&id) {
            pending.preview &= open.preview;
            pending.focus |= open.focus;
            return;
        }
        self.opening.insert(id.clone(), open);
        self.call_in(
            window,
            cx,
            move |api| Ok(api.resume(&dir)),
            move |this, resumed, window, cx| {
                let open = this.opening.remove(&id).unwrap_or(open);
                let opened = match resumed {
                    Ok(opened) => opened,
                    Err(error) => return this.report(format!("{error:#}"), cx),
                };
                let id = this.insert_chat(opened.session_dir, window, cx);
                this.show_tab(&id, open, window, cx);
            },
        );
    }

    /// Click on the session list: one click previews the chat, a double
    /// click keeps it and moves the keyboard there.
    pub(super) fn select_session(
        &mut self,
        dir: PathBuf,
        clicks: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keep = clicks > 1;
        let open = OpenDocument {
            preview: !keep,
            focus: keep,
        };
        self.open_chat(dir, open, window, cx);
    }

    pub(super) fn new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_settings(cx);
        let source = self.active_chat_dir().map(Path::to_path_buf);
        self.call_in(
            window,
            cx,
            move |api| {
                let opened = api.new_session(source.as_deref())?;
                Ok((opened, api.sessions()?))
            },
            |this, (opened, sessions), window, cx| {
                this.sessions = sessions;
                this.add_chat(opened.session_dir, window, cx);
            },
        );
    }

    /// Builds the chat of a session unless it exists; returns its tab id.
    fn insert_chat(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> String {
        let id = tabs::chat(&dir);
        if self.chats.contains_key(&id) {
            return id;
        }
        let Some(api) = self.api() else {
            return id;
        };
        let extras = self.composer_extras(cx);
        let view = cx.new(|cx| Chat::new(api, dir, extras, window, cx));
        let tab = id.clone();
        let events = cx.subscribe(&view, move |this, chat, event: &ChatEvent, cx| {
            this.on_chat_event(&tab, &chat, event, cx)
        });
        self.chats.insert(
            id.clone(),
            OpenChat {
                view,
                _events: events,
            },
        );
        id
    }

    /// Extension icons go under the composer of the active chat only.
    fn composer_extras(&self, cx: &Context<Self>) -> ComposerExtras {
        let workspace = cx.weak_entity();
        Rc::new(move |chat, cx| {
            let workspace = workspace.upgrade()?.read(cx);
            (workspace.active_chat_view()?.entity_id() == chat).then(|| {
                workspace
                    .render_badge_strip(BadgePlace::Composer, cx)
                    .into_any_element()
            })
        })
    }

    fn on_chat_event(
        &mut self,
        tab: &str,
        chat: &Entity<Chat>,
        event: &ChatEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            ChatEvent::RunSettled => {
                self.refresh_sessions(cx);
                if self.active_chat_view() == Some(chat) {
                    cx.emit(HostEvent::RunSettled);
                }
            }
            ChatEvent::Activity {
                session_dir,
                activity,
            } => {
                // Repeated copies from other streams carry no change.
                self.attend(session_dir, activity.status, cx);
                // Every session stream carries every update; only the active
                // chat's copy counts, and a switch reloads the list anyway.
                if self.active_chat.as_deref() != Some(tab) {
                    return;
                }
                match self
                    .sessions
                    .iter_mut()
                    .find(|summary| &summary.session_dir == session_dir)
                {
                    Some(summary) => summary.activity = Some(activity.clone()),
                    None => self.refresh_sessions(cx),
                }
            }
            ChatEvent::Sent => self.keep_tab(tab, cx),
            ChatEvent::ServerStopped => {
                self.status = Status::Failed("Сервер приложения остановлен".into());
            }
        }
        cx.notify();
    }

    /// Reloads the session list; requests made while one runs coalesce.
    pub(super) fn refresh_sessions(&mut self, cx: &mut Context<Self>) {
        if self.sessions_loading {
            self.sessions_stale = true;
            return;
        }
        self.sessions_loading = true;
        self.call(
            cx,
            |api| Ok(api.sessions()),
            |this, sessions, cx| {
                this.sessions_loading = false;
                match sessions {
                    Ok(sessions) => {
                        for summary in &sessions {
                            if let Some(activity) = &summary.activity {
                                this.attention
                                    .baseline(&summary.session_dir, activity.status);
                            }
                        }
                        this.sessions = sessions;
                    }
                    Err(error) => this.report(format!("{error:#}"), cx),
                }
                if std::mem::take(&mut this.sessions_stale) {
                    this.refresh_sessions(cx);
                }
            },
        );
    }

    /// Drops chats whose tabs closed and follows the layout to the active
    /// chat; extensions reread the session when it changes.
    pub(super) fn sync_chats(&mut self, layout: &TabLayout, cx: &mut Context<Self>) {
        self.chats.retain(|id, _| layout.contains(id));
        let active = layout
            .active_chat(self.active_chat.as_deref())
            .filter(|id| self.chats.contains_key(id));
        if active != self.active_chat {
            // Activity updates are taken from the active chat's stream; a
            // reload covers what arrived elsewhere meanwhile.
            if active.is_some() {
                self.notice = None;
                self.refresh_sessions(cx);
            }
            self.active_chat = active;
            cx.emit(HostEvent::SessionOpened);
            cx.notify();
        }
    }

    pub(super) fn focus_active_chat(&self, window: &mut Window, cx: &mut App) {
        if let Some(chat) = self.active_chat_view() {
            chat.update(cx, |chat, cx| chat.focus(window, cx));
        }
    }

    /// Shows a failed call in the active chat, or in the empty area.
    pub(super) fn report(&mut self, message: String, cx: &mut Context<Self>) {
        match self.active_chat_view() {
            Some(chat) => chat.update(cx, |chat, cx| chat.show_notice(message, cx)),
            None => self.notice = Some(message.into()),
        }
        cx.notify();
    }

    /// Profile settings changed; every chat rereads its session config.
    pub(super) fn reload_chat_configs(&self, cx: &mut Context<Self>) {
        for chat in self.chats.values() {
            chat.view.update(cx, |chat, cx| chat.load_config(cx));
        }
    }
}
