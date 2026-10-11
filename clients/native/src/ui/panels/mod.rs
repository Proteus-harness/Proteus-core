//! Panels: the session list and the panels of extensions. They are tabs
//! `panel:<name>` like chats and documents, opened pinned, by default in a
//! fixed pane on the left.

mod session_menu;
mod sessions;

pub(super) use session_menu::Renaming;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::*;
use gpui_kit::*;

use super::extensions::{self, Host, SurfaceView};
use super::workspace::Workspace;
use crate::tabs::{self, PaneId};

/// What the tab and the menus show for a panel.
pub(super) struct PanelInfo {
    pub id: String,
    pub title: &'static str,
    pub icon: Icon,
}

/// Panels that can be opened now: the session list, then the panels of
/// enabled extensions in catalog order.
pub(super) fn available(cx: &App) -> Vec<PanelInfo> {
    let sessions = info(&tabs::panel(tabs::SESSIONS), cx);
    let extensions = extensions::visible_panels(cx)
        .into_iter()
        .filter_map(|manifest| info(&tabs::panel(manifest.id), cx));
    sessions.into_iter().chain(extensions).collect()
}

/// The panel behind a tab id, when it can be shown.
pub(super) fn info(tab: &str, cx: &App) -> Option<PanelInfo> {
    if tabs::panel_name(tab)? == tabs::SESSIONS {
        return Some(PanelInfo {
            id: tab.to_string(),
            title: "Сессии",
            icon: Icon::new(Lucide::MessagesSquare),
        });
    }
    let manifest = extensions::panel(tab, cx)?;
    Some(PanelInfo {
        id: tab.to_string(),
        title: manifest.panel.as_ref()?.title,
        icon: Icon::new(manifest.icon),
    })
}

/// Tab id of the panel Ctrl+Shift+E goes to: the first extension panel.
pub(super) fn first_extension_panel(cx: &App) -> Option<String> {
    let manifest = extensions::visible_panels(cx).into_iter().next()?;
    Some(tabs::panel(manifest.id))
}

impl Workspace {
    /// The view of an extension panel, built on first use and kept while
    /// the window lives, so a reopened panel is as it was left.
    fn ensure_panel(
        &mut self,
        tab: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<SurfaceView> {
        let manifest = extensions::panel(tab, cx)?;
        if let Some(view) = self.panels.get(manifest.id) {
            return Some(view.clone());
        }
        let view =
            (manifest.panel.as_ref()?.build)(Host::new(cx.weak_entity(), manifest.id), window, cx);
        self.panels.insert(manifest.id, view.clone());
        Some(view)
    }

    pub(super) fn render_panel(
        &mut self,
        tab: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let content = if tabs::panel_name(tab) == Some(tabs::SESSIONS) {
            self.render_sessions(cx).into_any_element()
        } else {
            match self.ensure_panel(tab, window, cx) {
                Some(view) => view.view.into_any_element(),
                None => div().into_any_element(),
            }
        };
        let theme = cx.theme();
        div()
            .size_full()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .child(content)
            .into_any_element()
    }

    /// Moves the keyboard into a panel; the session list has no keys of
    /// its own, so the window takes them.
    pub(super) fn focus_panel(&mut self, tab: &str, window: &mut Window, cx: &mut Context<Self>) {
        match self.ensure_panel(tab, window, cx) {
            Some(view) => view.focus.focus(window, cx),
            None => self.focus_handle.focus(window, cx),
        }
    }

    /// Shows a panel where it is open, or opens it: in `pane`, or among
    /// the other panels; the keyboard goes there.
    pub(super) fn open_panel(
        &mut self,
        tab: &str,
        pane: Option<PaneId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if info(tab, cx).is_none() {
            return;
        }
        self.close_settings(cx);
        self.update_tabs(cx, |layout| layout.open_panel(tab, pane));
        self.focus_panel(tab, window, cx);
    }

    /// Ctrl+Shift+E and Ctrl+Shift+H: to the panel, and from it back to
    /// the active chat.
    pub(super) fn toggle_panel(&mut self, tab: &str, window: &mut Window, cx: &mut Context<Self>) {
        let layout = self.tab_layout(cx);
        let shown = layout.active(layout.focused) == Some(tab);
        let focused = match extensions::panel(tab, cx).and_then(|m| self.panels.get(m.id)) {
            Some(view) => view.focus.contains_focused(window, cx),
            None => true,
        };
        let back = self
            .active_chat
            .clone()
            .and_then(|chat| Some((layout.pane_of(&chat)?, chat)));
        match back {
            Some((pane, chat)) if shown && focused => self.activate_tab(pane, &chat, window, cx),
            _ => self.open_panel(tab, None, window, cx),
        }
    }

    /// Brings an extension forward: its panel.
    pub(super) fn activate_extension(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_panel(&tabs::panel(id), None, window, cx);
    }
}
