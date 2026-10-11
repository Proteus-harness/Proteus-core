use gpui_kit::component::message_scroller::MessageScroller;
use gpui_kit::component::*;
use gpui_kit::*;

use super::Chat;
use crate::ui::motion;
use crate::ui::prefs;

impl Render for Chat {
    /// The conversation: thread, pending requests, notice and composer.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let renderer = cx.processor(|this, ix: usize, window, cx| this.render_row(ix, window, cx));
        v_flex()
            .size_full()
            .child(motion::fade_in_if(
                MessageScroller::new("thread", self.scroller.clone(), renderer)
                    .flex_1()
                    .min_h_0()
                    .scrollbar(true)
                    .jump_button(true)
                    .with_row_style(StyleRefinement::default().px_0().pb_0()),
                "thread-enter",
                true,
            ))
            .children(self.render_requests(window, cx))
            .children(self.notice.clone().map(|notice| {
                h_flex().w_full().justify_center().px_6().pt_2().child(
                    div()
                        .w_full()
                        .max_w(prefs::chat_width(cx))
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(notice),
                )
            }))
            .child(self.render_composer(cx))
    }
}
