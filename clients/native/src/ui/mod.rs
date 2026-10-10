mod actions;
mod composer;
mod context_usage;
mod controls;
mod format;
mod motion;
mod panel;
mod requests;
mod settings;
mod sidebar;
mod thread;
mod tool_row;
mod user_input;
mod workspace;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::message_scroller::MessageScroller;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppRunStatus;

use workspace::Status;
pub use workspace::Workspace;

gpui_kit::actions!(
    proteus_native,
    [ToggleSettings, ToggleInfoPanel, NewSession]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-,", ToggleSettings, None),
        KeyBinding::new("ctrl-alt-b", ToggleInfoPanel, None),
        KeyBinding::new("ctrl-n", NewSession, None),
    ]);
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let root = h_flex()
            .size_full()
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
            }));
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
            Status::Ready => root.child(motion::rise_in(
                h_flex()
                    .size_full()
                    .child(self.render_sidebar(cx))
                    .child(self.render_main(window, cx))
                    .children(self.render_panel(cx)),
                "chat-screen",
            )),
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

    fn render_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_window_title(window);
        let main = v_flex().flex_1().min_w_0().h_full();
        let renderer = cx.processor(|this, ix: usize, window, cx| this.render_row(ix, window, cx));
        // A new id per session replays the fade when switching sessions.
        let thread_id = ElementId::NamedInteger("thread-enter".into(), self.session_key());
        main.child(self.render_header(cx))
            .child(motion::fade_in_if(
                MessageScroller::new("thread", self.scroller.clone(), renderer)
                    .flex_1()
                    .min_h_0()
                    .scrollbar(true)
                    .jump_button(true)
                    .with_row_style(StyleRefinement::default().px_0().pb_0()),
                thread_id,
                true,
            ))
            .children(self.render_requests(window, cx))
            .children(self.notice.clone().map(|notice| {
                h_flex().w_full().justify_center().px_6().pt_2().child(
                    div()
                        .w_full()
                        .max_w(px(thread::CONTENT_WIDTH))
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(notice),
                )
            }))
            .child(self.render_composer(cx))
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let session = self.session.as_ref();
        let title = self.session_title();
        let status = session.and_then(|session| {
            if !session.connected {
                return Some(match session.link_error {
                    Some(_) => ("Переподключение…", theme.warning),
                    None => ("Подключение…", theme.muted_foreground),
                });
            }
            let run = session.execution.active.as_ref()?;
            Some(match run.status {
                AppRunStatus::CancelRequested => ("Остановка…", theme.warning),
                _ => ("Работает…", theme.muted_foreground),
            })
        });
        h_flex()
            .h(px(44.))
            .flex_none()
            .px_4()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .font_semibold()
                    .truncate()
                    .child(title),
            )
            .children(status.map(|(label, color)| {
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(color)
                    .child(Spinner::new().xsmall())
                    .child(label)
            }))
            .child(
                Button::new("toggle-panel")
                    .ghost()
                    .small()
                    .icon(IconName::PanelRight)
                    .selected(self.panel.open)
                    .tooltip("Сведения о сессии")
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_panel(window, cx))),
            )
    }
}
