//! Blocks that fade in when they appear.

use std::time::Duration;

use gpui_kit::component::animation::EffectTransition;
use gpui_kit::*;

use super::{BASE, FAST, ease_enter, shift};

/// Rows of a list that join a staggered entrance; later ones appear at once.
const STAGGER_ROWS: usize = 14;
const STAGGER_STEP: Duration = Duration::from_millis(18);

/// Fade plus a short rise; for freshly shown blocks.
pub(in crate::ui) fn rise_in<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E> {
    EffectTransition::new(BASE)
        .ease(ease_enter)
        .slide_y(shift(6.), px(0.))
        .fade(0., 1.)
        .apply(element, id)
}

/// A short fade for a screen or page that replaces another: content that
/// moves on every switch reads as a jump.
pub(in crate::ui) fn switch_in<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E> {
    EffectTransition::new(FAST)
        .ease(ease_enter)
        .fade(0., 1.)
        .apply(element, id)
}

/// `rise_in` only when `play`, keeping the element ids below it stable
/// either way: a block shown again (another tab, a remount) stays put.
pub(in crate::ui) fn rise_in_if<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
    play: bool,
) -> AnimationElement<E> {
    let rise = shift(6.);
    element.with_animation(
        id,
        Animation::new(BASE).with_easing(ease_enter),
        move |element, delta| {
            if play {
                element.opacity(delta).top(rise * (1. - delta))
            } else {
                element
            }
        },
    )
}

/// Opacity only; safe inside virtualized lists that cache row heights.
///
/// The wrapper is always present so the element ids below it stay stable;
/// `play` decides whether this mount actually fades in. Virtualized rows
/// remount on scroll, so callers pass `play` only for fresh content.
pub(in crate::ui) fn fade_in_if<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
    play: bool,
) -> AnimationElement<E> {
    element.with_animation(
        id,
        Animation::new(BASE).with_easing(ease_enter),
        move |element, delta| {
            if play {
                element.opacity(delta)
            } else {
                element
            }
        },
    )
}

/// Row `index` of a list shown for the first time: rows fade in one after
/// another from the top. Pass `play` only while the list has just loaded,
/// so rows scrolled into view later do not animate.
pub(in crate::ui) fn stagger_in<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
    index: usize,
    play: bool,
) -> AnimationElement<E> {
    let play = play && index < STAGGER_ROWS;
    let delay = STAGGER_STEP * index.min(STAGGER_ROWS) as u32;
    let duration = FAST + delay;
    let total = duration.as_secs_f32();
    let (delay, fast) = (delay.as_secs_f32(), FAST.as_secs_f32());
    let rise = shift(4.);
    element.with_animation(
        id,
        Animation::new(duration)
            .with_easing(move |t| ease_enter(((t * total - delay) / fast).clamp(0., 1.))),
        move |element, delta| {
            if play {
                element.opacity(delta).top(rise * (1. - delta))
            } else {
                element
            }
        },
    )
}
