//! Short notes over the composer: a refused attachment, a server error.
//! A note stays a few seconds, longer for a longer text, or until closed.

use std::time::{Duration, Instant};

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::*;
use gpui_kit::*;

use super::Chat;
use crate::ui::{kit, motion, prefs};

const SHOWN: Duration = Duration::from_secs(4);
const PER_CHAR: Duration = Duration::from_millis(40);
const LONGEST: Duration = Duration::from_secs(12);

#[derive(Default)]
pub(super) struct Notice {
    text: Option<SharedString>,
    /// Counts the notes, so each one fades in afresh.
    count: usize,
    /// When the note came; shown again after a tab switch it stays put.
    shown_at: Option<Instant>,
    hide: Option<Task<()>>,
}

impl Chat {
    pub fn show_notice(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let text = text.into();
        let stay = (SHOWN + PER_CHAR * text.chars().count() as u32).min(LONGEST);
        self.notice.text = Some(text);
        self.notice.count += 1;
        self.notice.shown_at = Some(Instant::now());
        self.notice.hide = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(stay).await;
            this.update(cx, |this, cx| {
                this.notice.text = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(super) fn clear_notice(&mut self, cx: &mut Context<Self>) {
        self.notice.hide = None;
        if self.notice.text.take().is_some() {
            cx.notify();
        }
    }

    /// The note floats over the bottom of the thread, so the conversation
    /// does not jump when it comes and goes.
    pub(super) fn render_notice(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let text = self.notice.text.clone()?;
        let theme = cx.theme();
        let note = h_flex()
            .min_w_0()
            .max_w(prefs::chat_width(cx))
            .gap_1()
            .pl_3()
            .pr_1()
            .py_0p5()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_sm()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(div().flex_1().min_w_0().child(text))
            .child(
                kit::button("notice-close")
                    .icon(IconName::Close)
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(|this, _, _, cx| this.clear_notice(cx))),
            );
        Some(
            h_flex()
                .absolute()
                .bottom_2()
                .left_0()
                .right_0()
                .px_6()
                .justify_center()
                .child(motion::fade_in_if(
                    note,
                    ("notice", self.notice.count),
                    self.notice
                        .shown_at
                        .is_some_and(|at| at.elapsed() < motion::BASE * 2),
                )),
        )
    }
}
