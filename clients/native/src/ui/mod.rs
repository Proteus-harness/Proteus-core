mod actions;
mod badges;
mod composer;
mod context_usage;
mod controls;
mod extensions;
mod format;
mod motion;
mod panel;
mod prefs;
mod requests;
mod settings;
mod sidebar;
mod tabs;
mod thread;
mod tool_row;
mod user_input;
mod workspace;

use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::*;

use workspace::Status;
pub use workspace::Workspace;

gpui_kit::actions!(
    proteus_native,
    [
        ToggleSettings,
        ToggleInfoPanel,
        NewSession,
        NextSettingsPage,
        PreviousSettingsPage,
        NextTab,
        PreviousTab
    ]
);

pub(crate) const SETTINGS_CONTEXT: &str = "ProteusSettings";

pub fn init(cx: &mut App) {
    prefs::init(cx);
    cx.bind_keys([
        KeyBinding::new("ctrl-,", ToggleSettings, None),
        KeyBinding::new("ctrl-alt-b", ToggleInfoPanel, None),
        KeyBinding::new("ctrl-n", NewSession, None),
        KeyBinding::new("alt-down", NextSettingsPage, Some(SETTINGS_CONTEXT)),
        KeyBinding::new("alt-up", PreviousSettingsPage, Some(SETTINGS_CONTEXT)),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
    ]);
    extensions::bind_keys(cx);
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let root = h_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &ToggleSettings, window, cx| {
                if this.settings.is_some() {
                    this.close_settings(cx);
                    this.focus_composer(window, cx);
                } else {
                    this.open_settings(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleInfoPanel, window, cx| {
                if this.settings.is_none() {
                    this.toggle_panel(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NewSession, window, cx| {
                this.new_session(cx);
                this.focus_composer(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.step_tab(1, window, cx)))
            .on_action(
                cx.listener(|this, _: &PreviousTab, window, cx| this.step_tab(-1, window, cx)),
            )
            .on_drop(cx.listener(|this, _: &tabs::SplitDrag, _, cx| this.set_split(None, cx)));
        match &self.status {
            Status::Starting => root.child(centered(
                h_flex()
                    .gap_2()
                    .child(Spinner::new())
                    .child("Запуск Proteus…"),
            )),
            Status::Failed(error) => root.child(centered(
                v_flex()
                    .max_w(px(720.))
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
                self.sync_window_title(window);
                self.sync_badges(window, cx);
                root.child(motion::rise_in(
                    h_flex()
                        .size_full()
                        .child(self.render_sidebar(cx))
                        .child(self.render_tabs(window, cx))
                        .children(self.render_panel(window, cx)),
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
    fn session_title(&self) -> String {
        self.session
            .as_ref()
            .and_then(|session| {
                self.sessions
                    .iter()
                    .find(|summary| summary.session_dir == session.dir)
            })
            .map(format::session_title)
            .unwrap_or_else(|| "Новая сессия".to_owned())
    }

    fn session_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.session
            .as_ref()
            .map(|session| &session.dir)
            .hash(&mut hasher);
        hasher.finish()
    }

    fn sync_window_title(&mut self, window: &mut Window) {
        let title = format!("{} — Proteus", self.session_title());
        if self.window_title != title {
            window.set_window_title(&title);
            self.window_title = title;
        }
    }
}
