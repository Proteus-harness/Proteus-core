use gpui_kit::component::*;
use gpui_kit::*;

use super::Chat;
use super::scroller::ChatList;
use crate::ui::motion;
use crate::ui::prefs;

impl Render for Chat {
    /// The conversation: thread, pending requests, notice and composer.
    /// Files dropped anywhere on it attach to the draft.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let renderer = cx.processor(|this, ix: usize, window, cx| this.render_row(ix, window, cx));
        v_flex()
            .id("chat")
            .size_full()
            .drag_over::<ExternalPaths>(|chat, _, _, cx| chat.bg(cx.theme().drop_target))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.attach_files(paths.paths(), cx)
            }))
            .child(motion::fade_in_if(
                div().flex_1().min_h_0().child(ChatList::new(
                    "thread",
                    self.scroller.clone(),
                    renderer,
                )),
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
