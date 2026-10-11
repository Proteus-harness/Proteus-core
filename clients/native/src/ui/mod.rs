mod actions;
mod badges;
mod chat;
mod chats;
mod drag;
mod extensions;
mod format;
mod motion;
mod panels;
mod prefs;
mod settings;
mod tabs;
mod workspace;

use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::*;

use crate::tabs::Edge;
use workspace::Status;
pub use workspace::Workspace;

gpui_kit::actions!(
    proteus_native,
    [
        ToggleSettings,
        ToggleFiles,
        ToggleSessions,
        NewSession,
        NextSettingsPage,
        PreviousSettingsPage,
        NextTab,
        PreviousTab,
        SplitRight,
        SplitDown,
        FocusPaneLeft,
        FocusPaneRight,
        FocusPaneUp,
        FocusPaneDown,
        ZoomIn,
        ZoomOut,
        ZoomReset
    ]
);

pub(crate) const SETTINGS_CONTEXT: &str = "ProteusSettings";

pub fn init(cx: &mut App) {
    prefs::init(cx);
    cx.bind_keys([
        KeyBinding::new("ctrl-,", ToggleSettings, None),
        KeyBinding::new("ctrl-shift-e", ToggleFiles, None),
        KeyBinding::new("ctrl-shift-h", ToggleSessions, None),
        KeyBinding::new("ctrl-n", NewSession, None),
        KeyBinding::new("alt-down", NextSettingsPage, Some(SETTINGS_CONTEXT)),
        KeyBinding::new("alt-up", PreviousSettingsPage, Some(SETTINGS_CONTEXT)),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
        KeyBinding::new("ctrl-\\", SplitRight, None),
        // Shift+\ arrives as `|`.
        KeyBinding::new("ctrl-|", SplitDown, None),
        KeyBinding::new("ctrl-alt-left", FocusPaneLeft, None),
        KeyBinding::new("ctrl-alt-right", FocusPaneRight, None),
        KeyBinding::new("ctrl-alt-up", FocusPaneUp, None),
        KeyBinding::new("ctrl-alt-down", FocusPaneDown, None),
        KeyBinding::new("ctrl-=", ZoomIn, None),
        // Shift+= arrives as `+`.
        KeyBinding::new("ctrl-+", ZoomIn, None),
        KeyBinding::new("ctrl--", ZoomOut, None),
        KeyBinding::new("ctrl-0", ZoomReset, None),
    ]);
    extensions::bind_keys(cx);
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        motion::drive(window, cx);
        let theme = cx.theme();
        let root = h_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &ToggleSettings, window, cx| {
                if this.settings.is_some() {
                    this.close_settings(cx);
                    this.focus_active_chat(window, cx);
                } else {
                    this.open_settings(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleFiles, window, cx| {
                if this.settings.is_none()
                    && let Some(tab) = panels::first_extension_panel(cx)
                {
                    this.toggle_panel(&tab, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleSessions, window, cx| {
                if this.settings.is_none() {
                    let tab = crate::tabs::panel(crate::tabs::SESSIONS);
                    this.toggle_panel(&tab, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NewSession, window, cx| this.new_session(window, cx)))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.step_tab(1, window, cx)))
            .on_action(
                cx.listener(|this, _: &PreviousTab, window, cx| this.step_tab(-1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &SplitRight, window, cx| {
                this.split_active(Edge::Right, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SplitDown, window, cx| {
                this.split_active(Edge::Bottom, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusPaneLeft, window, cx| {
                this.focus_neighbor(Edge::Left, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusPaneRight, window, cx| {
                this.focus_neighbor(Edge::Right, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusPaneUp, window, cx| {
                this.focus_neighbor(Edge::Top, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusPaneDown, window, cx| {
                this.focus_neighbor(Edge::Bottom, window, cx)
            }))
            .on_action(|_: &ZoomIn, _, cx| prefs::update(cx, |prefs| prefs.view.zoom(1)))
            .on_action(|_: &ZoomOut, _, cx| prefs::update(cx, |prefs| prefs.view.zoom(-1)))
            .on_action(|_: &ZoomReset, _, cx| prefs::update(cx, |prefs| prefs.view.zoom(0)))
            .on_drop(cx.listener(|this, _: &tabs::SplitDrag, _, cx| this.finish_resize(cx)));
        match &self.status {
            Status::Starting => root.child(centered(
                h_flex()
                    .gap_2()
                    .child(Spinner::new())
                    .child("Запуск Proteus…"),
            )),
            Status::Failed(error) => root.child(centered(
                v_flex()
                    .max_w(prefs::scaled(720.))
                    .gap_2()
                    .child(div().font_semibold().child("Proteus недоступен"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(error.clone()),
                    ),
            )),
            Status::Ready if self.settings.is_some() => {
                let settings = self.settings.clone().expect("settings are open");
                root.child(motion::rise_in(
                    div().size_full().child(settings),
                    "settings-screen",
                ))
            }
            Status::Ready => {
                self.sync_window_title(window, cx);
                self.sync_badges(window, cx);
                let tabs = self.render_tabs(window, cx);
                root.child(motion::rise_in(
                    h_flex().size_full().child(tabs),
                    "chat-screen",
                ))
            }
        }
    }
}

fn centered(content: impl IntoElement) -> Div {
    h_flex()
        .size_full()
        .p_8()
        .items_center()
        .justify_center()
        .child(content)
}

impl Workspace {
    fn sync_window_title(&mut self, window: &mut Window, cx: &App) {
        let title = match self.active_chat_dir() {
            Some(dir) => format!("{} — Proteus", self.session_title(dir, cx)),
            None => "Proteus".to_owned(),
        };
        if self.window_title != title {
            window.set_window_title(&title);
            self.window_title = title;
        }
    }
}
