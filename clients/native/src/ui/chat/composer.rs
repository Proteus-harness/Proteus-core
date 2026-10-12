use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::Textarea;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppQueuedUserMessage;

use super::Chat;
use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::{format, prefs};

impl Chat {
    pub(super) fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let session = &self.session;
        let running = session.running();
        let ready = session.connected;
        let queued = session
            .pending
            .as_ref()
            .map(|pending| pending.queued_user_messages.as_slice())
            .unwrap_or_default();
        let start = (self.extras)(cx.entity_id(), false, cx);
        let end = (self.extras)(cx.entity_id(), true, cx);

        let keys = match prefs::current(cx).send {
            crate::preferences::SendMode::Enter => "Enter",
            crate::preferences::SendMode::CtrlEnter => "Ctrl+Enter",
        };
        let send = kit::button("send")
            .icon(IconName::ArrowUp)
            .hint_keys(
                if running {
                    "В очередь"
                } else {
                    "Отправить"
                },
                Some(keys),
                cx,
            )
            .primary()
            .small()
            .disabled(!ready)
            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)));
        let attach = kit::button("attach")
            .icon(IconName::Plus)
            .hint("Прикрепить изображения", cx)
            .ghost()
            .small()
            .on_click(cx.listener(|this, _, window, cx| this.choose_images(window, cx)));
        let chat = cx.entity().downgrade();
        let stop = running.then(|| {
            kit::button("stop")
                .icon(IconName::Square)
                .hint("Остановить", cx)
                .danger()
                .small()
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
        });

        h_flex()
            .w_full()
            .justify_center()
            .px_6()
            .pb_4()
            .pt_2()
            .child(
                v_flex()
                    .w_full()
                    .max_w(prefs::chat_width(cx))
                    .gap_1()
                    .p_2()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(theme.input)
                    // Raised off the conversation in either light or dark themes.
                    .bg(theme.background.blend(theme.foreground.opacity(0.04)))
                    .children(queued.iter().map(|message| render_queued(message, cx)))
                    .children(self.render_attachments(cx))
                    .child(
                        Textarea::new(&self.composer)
                            .text_size(prefs::chat_text(cx))
                            .appearance(false)
                            .bordered(false)
                            .on_paste(move |item, _, cx| {
                                chat.update(cx, |this, cx| this.attach_clipboard(item, cx))
                                    .unwrap_or(false)
                            }),
                    )
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_x_2()
                            .gap_y_1()
                            .child(attach)
                            .child(h_flex().flex_1().min_w_0().children(start))
                            .child(
                                h_flex()
                                    .ml_auto()
                                    .gap_2()
                                    .children(end)
                                    .children(stop)
                                    .child(send),
                            ),
                    ),
            )
    }
}

fn render_queued(message: &AppQueuedUserMessage, cx: &Context<Chat>) -> impl IntoElement {
    let theme = cx.theme();
    let message_id = message.message_id;
    let images = (message.image_count > 0).then(|| format!("+{} img", message.image_count));
    h_flex()
        .gap_2()
        .px_2()
        .py_1()
        .rounded(theme.radius)
        .bg(theme.muted)
        .text_xs()
        .child(
            Icon::new(IconName::Inbox)
                .xsmall()
                .text_color(theme.muted_foreground),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(format::clip(&message.text, 200)),
        )
        .when_some(images, |row, images| {
            row.child(div().text_color(theme.muted_foreground).child(images))
        })
        .child(
            kit::button(SharedString::from(format!("unqueue-{message_id}")))
                .icon(IconName::Close)
                .hint("Убрать из очереди", cx)
                .ghost()
                .xsmall()
                .on_click(cx.listener(move |this, _, _, cx| this.delete_queued(message_id, cx))),
        )
}
