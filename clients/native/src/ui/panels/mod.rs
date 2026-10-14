//! Panels: the session list and the panels of extensions, shown one at a
//! time in the side panel next to the rail.

mod header;
mod session_list;
mod session_menu;
mod sessions;
mod sidebar;

pub(super) use session_list::{SessionList, bind_keys};
pub(super) use session_menu::Renaming;
pub(super) use sidebar::SidebarDrag;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::*;
use gpui_kit::*;

use super::extensions::{self, Host, SurfaceView};
use super::prefs;
use super::workspace::Workspace;
use crate::preferences::SESSIONS_PANEL;

/// What the rail shows for a panel.
pub(super) struct PanelInfo {
    pub name: &'static str,
    pub title: &'static str,
    pub icon: Icon,
}

/// Panels that can be opened now: the session list, then the panels of
/// enabled extensions in catalog order.
pub(super) fn available(cx: &App) -> Vec<PanelInfo> {
    let sessions = info(SESSIONS_PANEL, cx);
    let extensions = extensions::visible_panels(cx)
        .into_iter()
        .filter_map(|manifest| info(manifest.id, cx));
    sessions.into_iter().chain(extensions).collect()
}

/// The panel called `name`, when it can be shown.
pub(super) fn info(name: &str, cx: &App) -> Option<PanelInfo> {
    if name == SESSIONS_PANEL {
        return Some(PanelInfo {
            name: SESSIONS_PANEL,
            title: "Сессии",
            icon: Icon::new(Lucide::MessagesSquare),
        });
    }
    let manifest = extensions::panel(name, cx)?;
    Some(PanelInfo {
        name: manifest.id,
        title: manifest.panel.as_ref()?.title,
        icon: Icon::new(manifest.icon),
    })
}

/// The panel Ctrl+Shift+E goes to: the first extension panel.
pub(super) fn first_extension_panel(cx: &App) -> Option<&'static str> {
    Some(extensions::visible_panels(cx).into_iter().next()?.id)
}

impl Workspace {
    /// The view of an extension panel, built on first use and kept while
    /// the window lives, so a reopened panel is as it was left.
    fn ensure_panel(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<SurfaceView> {
        let manifest = extensions::panel(name, cx)?;
        if let Some(view) = self.panels.get(manifest.id) {
            return Some(view.clone());
        }
        let view =
            (manifest.panel.as_ref()?.build)(Host::new(cx.weak_entity(), manifest.id), window, cx);
        self.panels.insert(manifest.id, view.clone());
        Some(view)
    }

    fn render_panel(
        &mut self,
        panel: &PanelInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let content = if panel.name == SESSIONS_PANEL {
            self.render_sessions(window, cx).into_any_element()
        } else {
            match self.ensure_panel(panel.name, window, cx) {
                Some(view) => view.view.into_any_element(),
                None => div().into_any_element(),
            }
        };
        let header = self.render_panel_header(panel, cx).into_any_element();
        v_flex()
            .size_full()
            .child(header)
            .child(div().flex_1().min_h_0().w_full().child(content))
            .into_any_element()
    }

    /// Moves the keyboard into a panel.
    fn focus_panel(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        if name == SESSIONS_PANEL {
            return self.session_list.focus.focus(window, cx);
        }
        match self.ensure_panel(name, window, cx) {
            Some(view) => view.focus.focus(window, cx),
            None => self.focus_handle.focus(window, cx),
        }
    }

    /// Whether the keyboard is in the panel `name`.
    fn panel_focused(&self, name: &str, window: &Window, cx: &App) -> bool {
        if name == SESSIONS_PANEL {
            return self.session_list.focus.contains_focused(window, cx);
        }
        match extensions::panel(name, cx).and_then(|m| self.panels.get(m.id)) {
            Some(view) => view.focus.contains_focused(window, cx),
            None => true,
        }
    }

    /// The panel in the side panel, when it is open and can be shown.
    pub(super) fn shown_panel(&self, cx: &App) -> Option<PanelInfo> {
        info(prefs::current(cx).sidebar.panel.as_deref()?, cx)
    }

    /// Shows a panel in the side panel; the keyboard goes there.
    pub(super) fn open_panel(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = info(name, cx) else {
            return;
        };
        self.close_settings(cx);
        prefs::update(cx, |prefs| {
            prefs.sidebar.panel = Some(panel.name.to_owned())
        });
        self.focus_panel(panel.name, window, cx);
    }

    /// A click on the icon of a panel in the rail: shows the panel, or
    /// closes the side panel when it already shows it.
    pub(super) fn press_panel(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let shown = self.shown_panel(cx).is_some_and(|panel| panel.name == name);
        if !shown || self.settings.is_some() {
            self.open_panel(name, window, cx);
            return;
        }
        let focused = self.panel_focused(name, window, cx);
        prefs::update(cx, |prefs| prefs.sidebar.panel = None);
        if focused {
            self.focus_active(window, cx);
        }
    }

    /// Ctrl+Shift+E and Ctrl+Shift+H: to the panel, and from it back to
    /// the active chat.
    pub(super) fn toggle_panel(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let shown = self.shown_panel(cx).is_some_and(|panel| panel.name == name);
        let layout = self.tab_layout(cx);
        let back = self
            .active_chat
            .clone()
            .and_then(|chat| Some((layout.pane_of(&chat)?, chat)));
        match back {
            Some((pane, chat)) if shown && self.panel_focused(name, window, cx) => {
                self.activate_tab(pane, &chat, window, cx)
            }
            _ => self.open_panel(name, window, cx),
        }
    }

    /// Brings an extension forward: its panel.
    pub(super) fn activate_extension(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_panel(id, window, cx);
    }
}
