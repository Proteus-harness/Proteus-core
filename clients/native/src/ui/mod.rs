mod actions;
mod analytics;
mod badges;
mod chat;
mod chats;
mod chrome;
mod drag;
mod extensions;
mod format;
mod keys;
mod kit;
mod launcher;
mod motion;
mod notifications;
mod panels;
mod prefs;
mod settings;
mod tabs;
mod theme;
mod tip;
mod wheel;
mod workspace;

use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::*;

use crate::tabs::Edge;
pub use launcher::Launcher;
use workspace::Status;
use workspace::Workspace;

gpui_kit::actions!(
    proteus_native,
    [
        ToggleSettings,
        ToggleFiles,
        ToggleSessions,
        NewSession,
        NewTerminal,
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
    theme::init(cx);
    // Command shortcuts are bound by `prefs::init` from the preferences.
    prefs::init(cx);
    cx.bind_keys([
        KeyBinding::new("alt-down", NextSettingsPage, Some(SETTINGS_CONTEXT)),
        KeyBinding::new("alt-up", PreviousSettingsPage, Some(SETTINGS_CONTEXT)),
    ]);
    extensions::bind_keys(cx);
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        motion::drive(window, cx);
        let theme = cx.theme();
        let root =
            v_flex()
                .size_full()
                .track_focus(&self.focus_handle)
                .bg(theme.background)
                .text_color(theme.foreground)
                .on_action(cx.listener(|this, _: &ToggleSettings, window, cx| {
                    this.toggle_settings(window, cx)
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
                .on_action(cx.listener(|this, _: &NewSession, window, cx| {
                    this.new_session(None, window, cx)
                }))
                .on_action(cx.listener(|this, _: &NewTerminal, window, cx| {
                    this.open_terminal(None, window, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &NextTab, window, cx| this.step_tab(1, window, cx)),
                )
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
        let root = root.child(self.render_top_bar(window, cx));
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
                            .text_color(cx.theme().muted_foreground)
                            .child(error.clone()),
                    ),
            )),
            Status::Ready if self.settings.is_some() => {
                let settings = self.settings.clone().expect("settings are open");
                root.child(motion::rise_in(
                    div().flex_1().min_h_0().w_full().child(settings),
                    "settings-screen",
                ))
            }
            Status::Ready => {
                self.sync_window_title(window, cx);
                self.sync_badges(window, cx);
                let status = self.render_status_bar(cx);
                let tabs = self.render_tabs(window, cx);
                root.child(motion::rise_in(
                    h_flex().flex_1().min_h_0().w_full().child(tabs),
                    "chat-screen",
                ))
                .child(status)
            }
        }
    }
}

fn centered(content: impl IntoElement) -> Div {
    h_flex()
        .flex_1()
        .min_h_0()
        .w_full()
        .p_8()
        .items_center()
        .justify_center()
        .child(content)
}

impl Workspace {
    fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            self.close_settings(cx);
            self.focus_active_chat(window, cx);
        } else {
            self.open_settings(window, cx);
        }
    }

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
