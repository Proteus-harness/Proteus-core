use gpui_kit::component::*;
use gpui_kit::*;

use super::Chat;
use super::scroller::ChatList;
use crate::ui::motion;

impl Render for Chat {
    /// The conversation: thread, pending requests and composer.
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
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(ChatList::new("thread", self.scroller.clone(), renderer))
                    .child(fade(cx.theme().background))
                    .children(self.render_notice(cx)),
                "thread-enter",
                true,
            ))
            .children(self.render_requests(window, cx))
            .child(self.render_composer(cx))
    }
}

/// The thread fades out above the composer instead of ending at a hard edge.
fn fade(background: Hsla) -> impl IntoElement {
    div()
        .absolute()
        .bottom_0()
        .left_0()
        .right_0()
        .h_8()
        .bg(linear_gradient(
            180.,
            linear_color_stop(background.opacity(0.), 0.),
            linear_color_stop(background, 1.),
        ))
}
