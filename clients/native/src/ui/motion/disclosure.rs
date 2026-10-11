//! Disclosures: chevrons that turn and content that grows or shrinks.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::time::Instant;

use gpui_kit::base::MotionReveal;
use gpui_kit::component::*;
use gpui_kit::*;

use super::{SLOW, Tween, ease_enter, moves};

const SPRING: SpringConfig = SpringConfig::new(420., 34., 1.);

/// Disclosure chevron that turns towards the open state.
pub(in crate::ui) fn chevron(id: impl Into<ElementId>, open: bool, color: Hsla) -> AnyElement {
    let icon = Icon::new(IconName::ChevronRight).xsmall().text_color(color);
    if !moves() {
        let angle = if open { FRAC_PI_2 } else { 0. };
        return icon.rotate(radians(angle)).into_any_element();
    }
    icon.with_spring(id, SpringAnimation::new(SPRING).to(open), |icon, phase| {
        icon.rotate(radians(phase.interpolate(0., FRAC_PI_2)))
    })
    .into_any_element()
}

/// Disclosures toggled a moment ago, keyed by caller-chosen names.
#[derive(Default)]
struct Toggles(HashMap<SharedString, Toggle>);

impl Global for Toggles {}

#[derive(Clone, Copy)]
struct Toggle {
    tween: Tween,
    from: f32,
    open: bool,
}

impl Toggle {
    fn progress(&self, now: Instant) -> f32 {
        let target = if self.open { 1. } else { 0. };
        self.from + (target - self.from) * ease_enter(self.tween.linear(now))
    }
}

/// Records that the disclosure `key` was just opened or closed, so its
/// content grows or shrinks instead of jumping. A toggle during a running
/// animation continues from the current height.
pub(in crate::ui) fn toggled(key: impl Into<SharedString>, open: bool, cx: &mut App) {
    let key = key.into();
    let animate = moves() && !cx.reduce_motion();
    let now = Instant::now();
    let toggles = cx.default_global::<Toggles>();
    toggles.0.retain(|_, toggle| toggle.tween.running(now));
    if !animate {
        toggles.0.remove(&key);
        return;
    }
    let from = toggles
        .0
        .get(&key)
        .map(|toggle| toggle.progress(now))
        .unwrap_or(if open { 0. } else { 1. });
    toggles.0.insert(
        key,
        Toggle {
            tween: Tween::start(SLOW),
            from,
            open,
        },
    );
}

/// Content of a disclosure. While `toggled` animates it, the content is
/// clipped to a growing or shrinking height and stays mounted until it has
/// closed; otherwise it is simply present when open.
pub(in crate::ui) fn reveal(
    key: impl Into<SharedString>,
    open: bool,
    cx: &App,
    content: impl FnOnce() -> AnyElement,
) -> Option<AnyElement> {
    let key = key.into();
    let now = Instant::now();
    let running = cx
        .try_global::<Toggles>()
        .and_then(|toggles| toggles.0.get(&key).copied())
        .filter(|toggle| toggle.open == open && toggle.tween.running(now));
    match running {
        // The reveal lays its child out on its own; without a full width the
        // child shrinks to its content and widens only once the reveal ends.
        Some(toggle) => Some(
            MotionReveal::new(
                ElementId::Name(key),
                toggle.progress(now),
                div().w_full().child(content()).into_any_element(),
            )
            .into_any_element(),
        ),
        None => open.then(content),
    }
}

/// Keeps a view redrawing while any disclosure animates; call from `render`.
pub(in crate::ui) fn drive(window: &mut Window, cx: &App) {
    let now = Instant::now();
    let running = cx
        .try_global::<Toggles>()
        .is_some_and(|toggles| toggles.0.values().any(|toggle| toggle.tween.running(now)));
    if running {
        window.request_animation_frame();
    }
}
