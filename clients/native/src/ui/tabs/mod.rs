//! Tabs of the window: chats, panels and extension documents (files) in a
//! tree of panes. The layout lives in the client preferences; this module
//! turns user actions into layout changes and keeps the focus where the
//! user is.

mod bar;
mod controls;
mod drop;
mod menu;
mod motion;
mod split;
mod tab;
mod view;

pub(super) use drop::TabDrop;
pub(super) use motion::TabMotion;
pub(super) use split::{SplitDrag, SplitResize};

use gpui_kit::*;

use super::extensions::{self, Host, OpenDocument, SurfaceView};
use super::workspace::Workspace;
use super::{panels, prefs};
use crate::tabs::{self, Edge, PaneId, TabLayout};

/// Width a pane needs before a new tab may split it.
const SPLIT_WIDTH: f32 = 760.;

impl Workspace {
    /// The saved layout without tabs whose extensions are gone, with the
    /// split being resized at its current size.
    pub(super) fn tab_layout(&self, cx: &App) -> TabLayout {
        let mut layout = prefs::current(cx).tabs.clone();
        layout.retain(|id| tab_available(id, cx));
        if let Some(resize) = &self.split_drag {
            layout.set_sizes(&resize.path, &resize.sizes);
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
        let old = layout.clone();
        change(&mut layout);
        self.tab_motion.record(&old, &layout);
        self.documents.retain(|id, _| layout.contains(id));
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
        let viewport = window.viewport_size();
        let scale = prefs::scale(cx);
        let rects = self.tab_layout(cx).rects_in(
            viewport.width.as_f32() / scale,
            viewport.height.as_f32() / scale,
        );
        let can_split = move |pane: PaneId| {
            rects
                .iter()
                .any(|(id, rect)| *id == pane && rect.w >= SPLIT_WIDTH)
        };
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

    pub(super) fn activate_tab(
        &mut self,
        pane: PaneId,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_tabs(cx, |tabs| tabs.activate(pane, id));
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
        pane: PaneId,
        before: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_tabs(cx, |tabs| tabs.move_tab(id, pane, before));
        self.focus_tab(id, window, cx);
    }

    /// Moves a tab into a new pane at `edge` of `pane`.
    pub(super) fn split_tab(
        &mut self,
        id: &str,
        pane: PaneId,
        edge: Edge,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_tabs(cx, |tabs| tabs.split(id, pane, edge));
        self.focus_tab(id, window, cx);
    }

    /// Splits the active tab of the focused pane off to `edge`.
    pub(super) fn split_active(&mut self, edge: Edge, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        self.update_tabs(cx, |tabs| tabs.split_active(edge));
        self.focus_active(window, cx);
    }

    pub(super) fn close_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        self.update_tabs(cx, |tabs| tabs.close_pane(pane));
        self.focus_active(window, cx);
    }

    pub(super) fn focus_pane(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        if prefs::current(cx).tabs.focused != pane {
            self.update_tabs(cx, |tabs| tabs.focus(pane));
        }
    }

    /// Moves the focus to the pane beyond `edge` of the focused one.
    pub(super) fn focus_neighbor(
        &mut self,
        edge: Edge,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.is_some() {
            return;
        }
        let mut next = None;
        self.update_tabs(cx, |tabs| next = tabs.focus_neighbor(edge));
        if let Some(id) = next {
            self.focus_tab(&id, window, cx);
        }
    }

    /// Cycles the tabs of the focused pane.
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

    /// Moves the keyboard to the active tab of the focused pane.
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
        } else if tabs::is_panel(id) {
            self.focus_panel(id, window, cx);
        } else if let Some(view) = self.ensure_document(id, window, cx) {
            view.focus.focus(window, cx);
        }
    }
}

/// Whether a tab id stands for something that can be shown. Chats whose
/// sessions are gone are closed when the workspace starts.
pub(super) fn tab_available(id: &str, cx: &App) -> bool {
    tabs::is_chat(id) || panels::info(id, cx).is_some() || extensions::document(id, cx).is_some()
}
