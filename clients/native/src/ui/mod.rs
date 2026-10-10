mod actions;
mod composer;
mod controls;
mod format;
mod requests;
mod sidebar;
mod thread;
mod tool_row;
mod user_input;
mod workspace;

use gpui_kit::component::message_scroller::MessageScroller;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppRunStatus;

use workspace::Status;
pub use workspace::Workspace;

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let root = h_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground);
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
            Status::Ready => root
                .child(self.render_sidebar(cx))
                .child(self.render_main(window, cx)),
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
    fn render_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let renderer = cx.processor(|this, ix: usize, window, cx| this.render_row(ix, window, cx));
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.render_header(cx))
            .child(
                MessageScroller::new("thread", self.scroller.clone(), renderer)
                    .flex_1()
                    .min_h_0()
                    .scrollbar(true)
                    .jump_button(true)
                    .with_row_style(StyleRefinement::default().px_0().pb_0()),
            )
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
        let title = session
            .and_then(|session| {
                self.sessions
                    .iter()
                    .find(|summary| summary.session_dir == session.dir)
            })
            .map(format::session_title)
            .unwrap_or_else(|| "Новая сессия".to_owned());
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
    }
}
