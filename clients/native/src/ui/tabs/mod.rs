//! Tabs of the main area: chats and extension documents (files) in one
//! or two groups. The layout lives in the client preferences; this module
//! turns user actions into layout changes and keeps the focus where the
//! user is.

mod bar;
mod view;

pub(super) use view::SplitDrag;

use gpui_kit::*;

use super::extensions::{self, Host, OpenDocument, SurfaceView};
use super::prefs;
use super::workspace::Workspace;
use crate::tabs::{self, TabLayout};

/// Width the main area needs before a new tab may split it.
const SPLIT_WIDTH: f32 = 760.;

impl Workspace {
    /// The saved layout without tabs whose extensions are gone.
    pub(super) fn tab_layout(&self, cx: &App) -> TabLayout {
        let mut layout = prefs::current(cx).tabs.clone();
        layout.retain(|id| tab_available(id, cx));
        if let Some(split) = self.split_drag {
            layout.set_split(split);
        }
        layout
    }

    pub(super) fn update_tabs(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut TabLayout),
    ) {
        let mut layout = prefs::current(cx).tabs.clone();
        layout.retain(|id| tab_available(id, cx));
        change(&mut layout);
        self.documents.retain(|id, _| layout.group_of(id).is_some());
        self.sync_chats(&layout, cx);
        prefs::update(cx, |prefs| prefs.tabs = layout);
        cx.notify();
    }

    /// The view of a document tab, built on first use.
    pub(super) fn ensure_document(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<SurfaceView> {
        if let Some(view) = self.documents.get(id) {
            return Some(view.clone());
        }
        let (manifest, argument) = extensions::document(id, cx)?;
        let surface = manifest.document.as_ref()?;
        let view = (surface.build)(Host::new(cx.weak_entity()), &argument, window, cx);
        self.documents.insert(id.to_string(), view.clone());
        Some(view)
    }

    /// Opens a document as a tab, or shows it where it is open.
    pub(super) fn open_document(
        &mut self,
        id: &str,
        open: OpenDocument,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyView> {
        let view = self.ensure_document(id, window, cx)?;
        self.close_settings(cx);
        self.show_tab(id, open, window, cx);
        Some(view.view)
    }

    /// Opens a tab whose page exists, or shows it where it is open. A
    /// preview tab is replaced by the next preview until it is kept.
    pub(super) fn show_tab(
        &mut self,
        id: &str,
        open: OpenDocument,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let can_split = self.main_width(window) >= px(SPLIT_WIDTH);
        self.update_tabs(cx, |tabs| {
            if open.preview {
                tabs.open_preview(id, can_split);
            } else {
                tabs.open(id, can_split);
                tabs.keep(id);
            }
        });
        if open.focus {
            self.focus_tab(id, window, cx);
        }
    }

    /// Rough width of the area right of the sidebar.
    fn main_width(&self, window: &Window) -> Pixels {
        window.viewport_size().width - px(super::sidebar::SIDEBAR_WIDTH)
    }

    pub(super) fn activate_tab(
        &mut self,
        group: usize,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_tabs(cx, |tabs| tabs.activate(group, id));
        self.focus_tab(id, window, cx);
    }

    /// Turns a preview tab into a regular one.
    pub(super) fn keep_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        self.update_tabs(cx, |tabs| tabs.keep(id));
    }

    pub(super) fn close_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.update_tabs(cx, |tabs| tabs.close(id));
        self.focus_active(window, cx);
    }

    pub(super) fn move_tab(
        &mut self,
        id: &str,
        group: usize,
        before: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_tabs(cx, |tabs| tabs.move_tab(id, group, before));
        self.focus_tab(id, window, cx);
    }

    pub(super) fn open_tab_in(
        &mut self,
        id: &str,
        group: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_tabs(cx, |tabs| tabs.open_in(id, group));
        self.focus_tab(id, window, cx);
    }

    pub(super) fn transfer_tab(
        &mut self,
        group: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_tabs(cx, |tabs| tabs.transfer(group));
        self.focus_active(window, cx);
    }

    pub(super) fn toggle_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update_tabs(cx, TabLayout::toggle_split);
        self.focus_active(window, cx);
    }

    pub(super) fn focus_group(&mut self, group: usize, cx: &mut Context<Self>) {
        if prefs::current(cx).tabs.focused != group {
            self.update_tabs(cx, |tabs| tabs.focus(group));
        }
    }

    /// Cycles the tabs of the focused group.
    pub(super) fn step_tab(&mut self, offset: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        let mut next = None;
        self.update_tabs(cx, |tabs| next = tabs.step(offset));
        if let Some(id) = next {
            self.focus_tab(&id, window, cx);
        }
    }

    pub(super) fn set_split(&mut self, split: Option<u16>, cx: &mut Context<Self>) {
        match split {
            Some(split) => {
                self.split_drag = Some(split);
                cx.notify();
            }
            None => {
                if let Some(split) = self.split_drag.take() {
                    self.update_tabs(cx, |tabs| tabs.set_split(split));
                }
            }
        }
    }

    pub(super) fn reset_split(&mut self, cx: &mut Context<Self>) {
        self.split_drag = None;
        self.update_tabs(cx, TabLayout::reset_split);
    }

    /// Moves the keyboard to the active tab of the focused group.
    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let layout = self.tab_layout(cx);
        if let Some(id) = layout.active(layout.focused).map(str::to_string) {
            self.focus_tab(&id, window, cx);
        }
    }

    fn focus_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if tabs::is_chat(id) {
            if let Some(chat) = self.chats.get(id) {
                chat.view.update(cx, |chat, cx| chat.focus(window, cx));
            }
        } else if let Some(view) = self.ensure_document(id, window, cx) {
            view.focus.focus(window, cx);
        }
    }

    /// Brings an extension forward: its view in the left sidebar.
    pub(super) fn activate_extension(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if extensions::find(id).is_some_and(|manifest| manifest.sidebar.is_some()) {
            self.close_settings(cx);
            self.show_sidebar(Some(id), window, cx);
        }
    }
}

/// Whether a tab id stands for something that can be shown. Chats whose
/// sessions are gone are closed when the workspace starts.
pub(super) fn tab_available(id: &str, cx: &App) -> bool {
    tabs::is_chat(id) || extensions::document(id, cx).is_some()
}
