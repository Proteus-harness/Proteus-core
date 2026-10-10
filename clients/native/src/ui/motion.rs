//! Shared motion: short enter transitions and spring-driven disclosure.
//!
//! Animation state lives with the element id, so an id that changes (another
//! page, a reopened section) plays the transition again.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::time::{Duration, Instant};

use gpui_kit::component::animation::{EffectTransition, ease_out_cubic};
use gpui_kit::component::*;
use gpui_kit::*;

const ENTER: Duration = Duration::from_millis(180);
const SPRING: SpringConfig = SpringConfig::new(420., 34., 1.);

/// Fade plus a short rise; for pages and freshly shown blocks.
pub(super) fn rise_in<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E> {
    EffectTransition::new(ENTER)
        .ease(ease_out_cubic)
        .slide_y(px(6.), px(0.))
        .fade(0., 1.)
        .apply(element, id)
}

/// Opacity only; safe inside virtualized lists that cache row heights.
///
/// The wrapper is always present so the element ids below it stay stable;
/// `play` decides whether this mount actually fades in. Virtualized rows
/// remount on scroll, so callers pass `play` only for fresh content.
pub(super) fn fade_in_if<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
    play: bool,
) -> AnimationElement<E> {
    element.with_animation(
        id,
        Animation::new(ENTER).with_easing(ease_out_cubic),
        move |element, delta| {
            if play {
                element.opacity(delta)
            } else {
                element
            }
        },
    )
}

/// Bottom bar entering from below.
pub(super) fn slide_up<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E> {
    EffectTransition::new(Duration::from_millis(220))
        .ease(ease_out_cubic)
        .slide_y(px(16.), px(0.))
        .fade(0., 1.)
        .apply(element, id)
}

/// Disclosure chevron that turns towards the open state.
pub(super) fn chevron(id: impl Into<ElementId>, open: bool, color: Hsla) -> impl IntoElement {
    Icon::new(IconName::ChevronRight)
        .xsmall()
        .text_color(color)
        .with_spring(id, SpringAnimation::new(SPRING).to(open), |icon, phase| {
            icon.rotate(radians(phase.interpolate(0., FRAC_PI_2)))
        })
}

/// Width spring for side panels; `open` drives 0 → `width`.
pub(super) fn reveal_width<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
    open: bool,
    width: Pixels,
) -> impl IntoElement {
    element.with_spring(
        id,
        SpringAnimation::new(SPRING).to(open).from(false),
        move |element, phase| element.w(phase.clamp(0.0..=1.0).interpolate(px(0.), width)),
    )
}

/// Keys that changed a moment ago: lets list rows animate only right after
/// they appeared or toggled, not every time they scroll back into view.
#[derive(Default)]
pub(super) struct Recent {
    marks: HashMap<u64, Instant>,
}

impl Recent {
    const WINDOW: Duration = Duration::from_millis(400);

    pub(super) fn mark(&mut self, key: u64) {
        let now = Instant::now();
        self.marks
            .retain(|_, at| now.duration_since(*at) < Self::WINDOW);
        self.marks.insert(key, now);
    }

    pub(super) fn is_fresh(&self, key: u64) -> bool {
        self.marks
            .get(&key)
            .is_some_and(|at| at.elapsed() < Self::WINDOW)
    }

    pub(super) fn clear(&mut self) {
        self.marks.clear();
    }
}
