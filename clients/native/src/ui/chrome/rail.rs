//! The rail along the left edge, under the top bar: the panels it opens
//! beside itself, the narrow extension icons placed at either end, and the
//! settings at the bottom.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::preferences::{BadgePlace, SESSIONS_PANEL};
use crate::shortcuts::Command;
use crate::ui::kit::Pressable as _;
use crate::ui::tip::Tip as _;
use crate::ui::workspace::Workspace;
use crate::ui::{kit, panels, prefs};

/// Rail width before zoom.
const RAIL_WIDTH: f32 = 40.;

impl Workspace {
    pub(in crate::ui) fn render_rail(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        v_flex()
            .flex_none()
            .w(prefs::scaled(RAIL_WIDTH))
            .h_full()
            .py_1()
            .gap_1()
            .items_center()
            .bg(theme.status_bar)
            .border_r_1()
            .border_color(theme.status_bar_border)
            .text_color(theme.muted_foreground)
            .child(self.render_panel_buttons(cx))
            .child(self.render_badge_strip(BadgePlace::RailStart, cx))
            .child(div().flex_1())
            .child(self.render_badge_strip(BadgePlace::RailEnd, cx))
            .child(self.render_settings_button(cx))
    }

    /// An icon for each panel; the one shown beside the rail is marked.
    fn render_panel_buttons(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let shown = (self.settings.is_none())
            .then(|| self.shown_panel(cx))
            .flatten()
            .map(|panel| panel.name);
        let files = panels::first_extension_panel(cx);
        let buttons = panels::available(cx).into_iter().map(|panel| {
            let name = panel.name;
            let command = if name == SESSIONS_PANEL {
                Some(Command::ToggleSessions)
            } else if Some(name) == files {
                Some(Command::ToggleFiles)
            } else {
                None
            };
            let keys = command.and_then(|command| kit::command_keys(command, cx));
            let hint = kit::hint_text(panel.title, keys.as_deref(), cx);
            rail_button(
                ElementId::Name(format!("rail-panel-{name}").into()),
                panel.icon,
                shown == Some(name),
                hint,
                cx,
            )
            .on_click(cx.listener(move |this, _, window, cx| this.press_panel(name, window, cx)))
        });
        v_flex().gap_1().items_center().children(buttons)
    }

    fn render_settings_button(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let keys = kit::command_keys(Command::ToggleSettings, cx);
        let hint = kit::hint_text("Настройки", keys.as_deref(), cx);
        rail_button(
            "rail-settings",
            Icon::new(Lucide::Settings),
            self.settings.is_some(),
            hint,
            cx,
        )
        .on_click(cx.listener(|this, _, window, cx| this.toggle_settings(window, cx)))
    }
}

/// A button of the rail, marked while what it opens is shown.
fn rail_button(
    id: impl Into<ElementId>,
    icon: Icon,
    active: bool,
    hint: Option<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let (hover, foreground) = (theme.list_hover, theme.foreground);
    div()
        .id(id)
        .size_8()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.radius)
        .cursor_pointer()
        .pressable(cx)
        .when(active, |button| {
            button.bg(theme.accent).text_color(theme.accent_foreground)
        })
        .when(!active, |button| {
            button.hover(move |style| style.bg(hover).text_color(foreground))
        })
        .child(icon)
        .when_some(hint, |button, hint| button.tip_text(hint))
}
