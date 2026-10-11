//! Session chosen for the usage and turn pages, shared between them and
//! independent of the active chat.

use std::path::PathBuf;

use gpui_kit::*;
use proteus_contracts::app_protocol::AppSessionSummary;

use super::widgets::{Choice, select};
use crate::api::Api;
use crate::ui::format;

pub(super) struct SessionPicker {
    api: Api,
    pub sessions: Vec<AppSessionSummary>,
    pub selected: PathBuf,
}

impl SessionPicker {
    pub fn new(api: Api, selected: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut picker = Self {
            api,
            sessions: Vec::new(),
            selected,
        };
        picker.reload(cx);
        picker
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let api = self.api.clone();
        let task = cx.background_spawn(async move { api.sessions() });
        cx.spawn(async move |this, cx| {
            if let Ok(sessions) = task.await {
                this.update(cx, |this, cx| {
                    this.sessions = sessions;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub fn current(&self) -> Option<&AppSessionSummary> {
        self.sessions
            .iter()
            .find(|session| session.session_dir == self.selected)
    }

    pub fn title(&self) -> String {
        self.current()
            .map(format::session_title)
            .unwrap_or_else(|| "Сохранённая сессия".into())
    }

    pub fn select(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if dir != self.selected {
            self.selected = dir;
            cx.notify();
        }
    }

    /// Dropdown over saved sessions, newest first.
    pub fn render(picker: &Entity<Self>, cx: &App) -> AnyElement {
        let this = picker.read(cx);
        let choices = this
            .sessions
            .iter()
            .map(|session| Choice {
                label: format!(
                    "{} · {} сообщ.",
                    format::session_title(session),
                    session.message_count
                )
                .into(),
                value: session.session_dir.clone(),
                checked: session.session_dir == this.selected,
            })
            .collect();
        let picker = picker.downgrade();
        select(
            "analytics-session",
            this.title(),
            choices,
            move |dir, _, cx| {
                picker.update(cx, |picker, cx| picker.select(dir, cx)).ok();
            },
        )
    }
}
