//! Tabs of the main area: the chat and extension pages in one or two
//! groups. The layout lives in the client preferences; this module turns
//! user actions into layout changes and keeps the focus where the user is.

mod bar;
mod view;

pub(super) use view::SplitDrag;

use gpui_kit::*;

use super::extensions::{self, Host, PageView};
use super::prefs;
use super::workspace::Workspace;
use crate::tabs::{CHAT, TabLayout};

/// Width the main area needs before a new tab may split it.
const SPLIT_WIDTH: f32 = 760.;

impl Workspace {
    /// The saved layout without tabs whose pages are gone.
    pub(super) fn tab_layout(&self, cx: &App) -> TabLayout {
        let mut layout = prefs::current(cx).tabs.clone();
        layout.retain(|id| page_available(id, cx));
        if let Some(split) = self.split_drag {
            layout.set_split(split);
        }
        layout
    }

    fn update_tabs(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut TabLayout)) {
        let mut layout = prefs::current(cx).tabs.clone();
        layout.retain(|id| page_available(id, cx));
        change(&mut layout);
        prefs::update(cx, |prefs| prefs.tabs = layout);
        cx.notify();
    }

    /// The page of an enabled extension, built on first use.
    pub(super) fn ensure_page(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PageView> {
        let manifest = extensions::find(id)?;
        let surface = manifest.page.as_ref()?;
        if !extensions::is_enabled(id, cx) {
            return None;
        }
        if let Some(page) = self.pages.get(manifest.id) {
            return Some(page.clone());
        }
        let page = (surface.build)(Host::new(cx.weak_entity()), window, cx);
        self.pages.insert(manifest.id, page.clone());
        Some(page)
    }

    /// Opens an extension page as a tab, or shows it where it is open.
    pub(super) fn open_tab(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyView> {
        let page = self.ensure_page(id, window, cx)?;
        self.close_settings(cx);
        let can_split = self.main_width(window) >= px(SPLIT_WIDTH);
        self.update_tabs(cx, |tabs| tabs.open(id, can_split));
        page.focus.focus(window, cx);
        Some(page.view)
    }

    /// Rough width of the area between the sessions list and the panel.
    fn main_width(&self, window: &Window) -> Pixels {
        let panel = if self.panel.open {
            super::panel::PANEL_WIDTH
        } else {
            0.
        };
        window.viewport_size().width - px(super::sidebar::SIDEBAR_WIDTH + panel)
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
        if id == CHAT {
            self.focus_composer(window, cx);
        } else if let Some(page) = self.ensure_page(id, window, cx) {
            page.focus.focus(window, cx);
        }
    }

    /// Brings an extension forward: its tab, or else its panel widget.
    pub(super) fn activate_extension(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(manifest) = extensions::find(id) else {
            return;
        };
        if manifest.page.is_some() {
            self.open_tab(id, window, cx);
        } else if manifest.panel.is_some() {
            self.show_widget(id, window, cx);
        }
    }

    pub(super) fn show_widget(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_settings(cx);
        if prefs::current(cx).extensions.is_collapsed(id) {
            prefs::update(cx, |prefs| prefs.extensions.toggle_collapsed(id));
        }
        if !self.panel.open {
            self.toggle_panel(window, cx);
        }
        cx.notify();
    }
}

/// Whether a tab id stands for something that can be shown.
pub(super) fn page_available(id: &str, cx: &App) -> bool {
    id == CHAT
        || extensions::find(id)
            .is_some_and(|manifest| manifest.page.is_some() && extensions::is_enabled(id, cx))
}
