use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppQueuedUserMessage;

use super::Chat;
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
        let extras = (self.extras)(cx.entity_id(), cx);

        let keys = match prefs::current(cx).send {
            crate::preferences::SendMode::Enter => "Enter",
            crate::preferences::SendMode::CtrlEnter => "Ctrl+Enter",
        };
        let send = Button::new("send")
            .icon(IconName::ArrowUp)
            .tooltip(if running {
                format!("В очередь ({keys})")
            } else {
                format!("Отправить ({keys})")
            })
            .primary()
            .small()
            .disabled(!ready)
            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)));
        let stop = running.then(|| {
            Button::new("stop")
                .icon(IconName::Square)
                .tooltip("Остановить")
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
                    .rounded(theme.radius * 3.)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .children(queued.iter().map(|message| render_queued(message, cx)))
                    .child(
                        Textarea::new(&self.composer)
                            .text_size(prefs::chat_text(cx))
                            .appearance(false)
                            .bordered(false),
                    )
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_x_2()
                            .gap_y_1()
                            .child(div().flex_1().children(self.render_controls(cx)))
                            .child(
                                h_flex()
                                    .ml_auto()
                                    .gap_2()
                                    .children(extras)
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
            Button::new(SharedString::from(format!("unqueue-{message_id}")))
                .icon(IconName::Close)
                .tooltip("Убрать из очереди")
                .ghost()
                .xsmall()
                .on_click(cx.listener(move |this, _, _, cx| this.delete_queued(message_id, cx))),
        )
}
