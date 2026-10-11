//! Smooth mouse wheel. A notched wheel arrives as whole lines and gpui
//! scrolls them in one jump; the window root takes those events and replays
//! them as pixel steps over a few frames, so whatever scrolls under the
//! pointer glides as in a browser. Touchpad scrolling already comes in
//! pixels and passes through untouched, as does every wheel when motion is
//! off.

use std::cell::RefCell;
use std::time::Instant;

use gpui_kit::*;

use super::motion;

/// Time constant of the glide: the remaining distance shrinks by ~63% per τ,
/// so a notch settles in about 0.2 s.
const GLIDE_SECONDS: f32 = 0.07;
const SETTLED: Pixels = px(0.5);

#[derive(Default)]
struct Glide {
    remaining: Point<Pixels>,
    modifiers: Modifiers,
    last_frame: Option<Instant>,
    running: bool,
}

thread_local! {
    static GLIDE: RefCell<Glide> = RefCell::default();
}

/// An invisible layer for the window root that turns wheel lines into a glide.
pub(super) fn layer() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |_, _, window, _| {
            window.on_mouse_event(|event: &ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture || !motion::plays() {
                    return;
                }
                if !matches!(event.delta, ScrollDelta::Lines(_)) {
                    return;
                }
                cx.stop_propagation();
                let delta = event.delta.pixel_delta(window.line_height());
                let start = GLIDE.with_borrow_mut(|glide| {
                    glide.remaining += delta;
                    glide.modifiers = event.modifiers;
                    !std::mem::replace(&mut glide.running, true)
                });
                if start {
                    GLIDE.with_borrow_mut(|glide| glide.last_frame = Some(Instant::now()));
                    window.on_next_frame(step);
                }
            });
        },
    )
    .absolute()
    .size_full()
}

/// Scrolls by this frame's share of the remaining distance.
fn step(window: &mut Window, cx: &mut App) {
    let now = Instant::now();
    let (delta, modifiers, more) = GLIDE.with_borrow_mut(|glide| {
        let dt = glide
            .last_frame
            .map_or(1. / 60., |at| now.duration_since(at).as_secs_f32().min(0.05));
        glide.last_frame = Some(now);
        let share = 1. - (-dt / GLIDE_SECONDS).exp();
        let mut delta = point(glide.remaining.x * share, glide.remaining.y * share);
        let left = glide.remaining - delta;
        if left.x.abs() < SETTLED && left.y.abs() < SETTLED {
            delta = glide.remaining;
            glide.remaining = Point::default();
            glide.running = false;
        } else {
            glide.remaining = left;
        }
        (delta, glide.modifiers, glide.running)
    });
    window.dispatch_event(
        PlatformInput::ScrollWheel(ScrollWheelEvent {
            position: window.mouse_position(),
            delta: ScrollDelta::Pixels(delta),
            modifiers,
            touch_phase: TouchPhase::Moved,
        }),
        cx,
    );
    if more {
        window.on_next_frame(step);
    }
}
