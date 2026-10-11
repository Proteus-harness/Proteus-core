//! Sliders for chat text size and width; each move applies and stores it.

use gpui_kit::component::h_flex;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::*;

use super::SettingsView;
use crate::preferences::{CHAT_TEXT, CHAT_WIDTH, Preferences};
use crate::ui::prefs;

pub(super) struct SizeSliders {
    pub chat_text: Entity<SliderState>,
    pub chat_width: Entity<SliderState>,
}

impl SizeSliders {
    pub fn new(window: &mut Window, cx: &mut Context<SettingsView>) -> (Self, Vec<Subscription>) {
        let view = prefs::current(cx).view;
        let chat_text = cx.new(|_| {
            SliderState::new()
                .max(f32::from(*CHAT_TEXT.end()))
                .min(f32::from(*CHAT_TEXT.start()))
                .step(1.)
                .default_value(f32::from(view.chat_text))
        });
        let chat_width = cx.new(|_| {
            SliderState::new()
                .max(f32::from(*CHAT_WIDTH.end()))
                .min(f32::from(*CHAT_WIDTH.start()))
                .step(10.)
                .default_value(f32::from(view.chat_width))
        });
        let subscriptions = vec![
            follow(&chat_text, window, cx, |prefs, value| {
                prefs.view.chat_text = value
            }),
            follow(&chat_width, window, cx, |prefs, value| {
                prefs.view.chat_width = value
            }),
        ];
        (
            Self {
                chat_text,
                chat_width,
            },
            subscriptions,
        )
    }
}

fn follow(
    slider: &Entity<SliderState>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    apply: fn(&mut Preferences, u16),
) -> Subscription {
    cx.subscribe_in(slider, window, move |_, _, event: &SliderEvent, _, cx| {
        let (SliderEvent::Change(value) | SliderEvent::Release(value)) = event;
        let value = value.end().round() as u16;
        prefs::update(cx, |prefs| apply(prefs, value));
        cx.notify();
    })
}

/// The slider with its current value on the right.
pub(super) fn slider(state: &Entity<SliderState>, unit: &'static str, cx: &App) -> AnyElement {
    let value = state.read(cx).value().end().round() as u16;
    h_flex()
        .gap_3()
        .child(div().w(prefs::scaled(180.)).child(Slider::new(state)))
        .child(
            div()
                .w(prefs::scaled(64.))
                .text_sm()
                .text_right()
                .child(format!("{value} {unit}")),
        )
        .into_any_element()
}
