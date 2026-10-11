//! Reasoning effort as a row of steps: a notch per effort on one line, the
//! name under each. A click or a drag along the line picks a step; the
//! session hears about it when the button is released.

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::config::ConfigSummary;

use super::{SelectorBadge, effort_position};
use crate::ui::chat::Chat;
use crate::ui::chat::controls::{current_effort, efforts};
use crate::ui::prefs;

/// Height of the line with the notches.
const TRACK: f32 = 18.;
const NOTCH: f32 = 8.;
const THUMB: f32 = 14.;
const LINE: f32 = 2.;

impl SelectorBadge {
    pub(super) fn effort_section(
        &self,
        config: &ConfigSummary,
        chat: WeakEntity<Chat>,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (muted, primary) = (theme.muted_foreground, theme.primary);
        let inherited = current_effort(config).is_none();
        let shown = self
            .scrub
            .or(self.pending)
            .unwrap_or_else(|| effort_position(config));
        let reset = div()
            .id("effort-profile")
            .text_xs()
            .cursor_pointer()
            .text_color(if inherited && self.scrub.is_none() {
                primary
            } else {
                muted
            })
            .hover(move |style| style.text_color(primary))
            .child("по профилю")
            .on_click(move |_, _, cx| {
                let _ = chat.update(cx, |chat, cx| chat.set_effort(None, cx));
            });
        let names = efforts(config);
        let last = names.len().saturating_sub(1);
        let this = cx.entity().downgrade();
        let release = move |_: &MouseUpEvent, _: &mut Window, cx: &mut App| {
            let _ = this.update(cx, |this, cx| this.finish_scrub(cx));
        };
        let steps = names
            .iter()
            .enumerate()
            .map(|(index, name)| self.step(index, last, shown, name, cx));
        v_flex()
            .px_2()
            .pt_1()
            .pb_1p5()
            .gap_1()
            .child(
                h_flex()
                    .justify_between()
                    .child(div().text_xs().text_color(muted).child("Рассуждения"))
                    .child(reset),
            )
            .child(
                h_flex()
                    .id("effort-steps")
                    .items_start()
                    .on_mouse_up(MouseButton::Left, release.clone())
                    .on_mouse_up_out(MouseButton::Left, release)
                    .children(steps),
            )
    }

    /// One effort: its piece of the line, the notch and the name under it.
    fn step(
        &self,
        index: usize,
        last: usize,
        shown: usize,
        name: &str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let (muted, primary, rest, ring) = (
            theme.muted_foreground,
            theme.primary,
            theme.border,
            theme.popover,
        );
        let half = |left: bool| {
            let filled = if left { index <= shown } else { index < shown };
            div()
                .absolute()
                .top(prefs::scaled((TRACK - LINE) / 2.))
                .h(prefs::scaled(LINE))
                .w_1_2()
                .when(left, |line| line.left_0())
                .when(!left, |line| line.right_0())
                .bg(if filled { primary } else { rest })
        };
        let current = index == shown;
        let size = prefs::scaled(if current { THUMB } else { NOTCH });
        let notch = div()
            .size(size)
            .rounded_full()
            .bg(if index <= shown { primary } else { rest })
            .when(current, |thumb| thumb.border_2().border_color(ring));
        let this = cx.entity().downgrade();
        let press = this.clone();
        v_flex()
            .id(("effort-step", index))
            .flex_1()
            .items_center()
            .gap_0p5()
            .cursor_pointer()
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(prefs::scaled(TRACK))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(index > 0, |track| track.child(half(true)))
                    .when(index < last, |track| track.child(half(false)))
                    .child(notch),
            )
            .child(
                div()
                    .px_1()
                    .text_xs()
                    .whitespace_nowrap()
                    .text_color(if current { primary } else { muted })
                    .child(name.to_owned()),
            )
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                let _ = press.update(cx, |this, cx| this.scrub_to(index, cx));
            })
            .on_mouse_move(move |event, _, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    let _ = this.update(cx, |this, cx| {
                        if this.scrub.is_some() {
                            this.scrub_to(index, cx);
                        }
                    });
                }
            })
    }
}
