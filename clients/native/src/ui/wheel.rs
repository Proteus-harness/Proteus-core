//! Smooth scrolling. A notched wheel arrives as whole lines and gpui
//! scrolls them in one jump; the window root takes those events and replays
//! them as pixel steps over a few frames, so whatever scrolls under the
//! pointer glides as in a browser. Touchpad scrolling comes in pixels and
//! passes through, then keeps gliding after a flick (`fling`). Nothing is
//! smoothed when motion is off. Holding Ctrl scrolls several times faster.

mod fling;

use std::cell::{Cell, RefCell};
use std::time::Instant;

use gpui_kit::*;

use super::motion;

/// Time constant of the glide: the remaining distance shrinks by ~63% per τ,
/// so a notch settles in about 0.2 s.
const GLIDE_SECONDS: f32 = 0.07;
const SETTLED: Pixels = px(0.5);
/// How much faster scrolling goes while Ctrl is held.
const FAST: f32 = 5.;

#[derive(Default)]
struct Glide {
    remaining: Point<Pixels>,
    modifiers: Modifiers,
    last_frame: Option<Instant>,
    running: bool,
}

thread_local! {
    static GLIDE: RefCell<Glide> = RefCell::default();
    /// Set while this layer dispatches its own steps, which pass through it.
    static REPLAYING: Cell<bool> = const { Cell::new(false) };
}

/// An invisible layer for the window root that turns wheel lines into a glide,
/// lets touchpad flicks coast and speeds up any scrolling while Ctrl is held.
/// It goes before the content: capture handlers run in paint order, and a
/// scroller that consumes the wheel there (gpui-kit's `ScrollableMask` over
/// the chat list) would otherwise hide the gesture from the layer.
pub(super) fn layer() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |_, _, window, _| {
            window.on_mouse_event(|_: &MouseDownEvent, phase, _, _| {
                if phase == DispatchPhase::Capture {
                    fling::stop();
                }
            });
            window.on_mouse_event(|event: &ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture || REPLAYING.get() {
                    return;
                }
                let fast = event.modifiers.control;
                // Replayed events go out without Ctrl.
                let modifiers = Modifiers {
                    control: false,
                    ..event.modifiers
                };
                let speed = if fast { FAST } else { 1. };
                match event.delta {
                    ScrollDelta::Pixels(pixels) if coasts() => {
                        fling::track(event.position, pixels * speed, modifiers, window);
                    }
                    _ => fling::stop(),
                }
                let lines = matches!(event.delta, ScrollDelta::Lines(_));
                let glide = lines && motion::plays();
                if !glide && !fast {
                    return;
                }
                cx.stop_propagation();
                if !glide {
                    let delta = match event.delta {
                        ScrollDelta::Lines(lines) => ScrollDelta::Lines(lines * speed),
                        ScrollDelta::Pixels(pixels) => ScrollDelta::Pixels(pixels * speed),
                    };
                    let replayed = ScrollWheelEvent {
                        delta,
                        modifiers,
                        ..event.clone()
                    };
                    window.defer(cx, move |window, cx| replay(replayed, window, cx));
                    return;
                }
                let delta = event.delta.pixel_delta(window.line_height()) * speed;
                let start = GLIDE.with_borrow_mut(|glide| {
                    glide.remaining += delta;
                    glide.modifiers = modifiers;
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

/// Touchpad inertia is added where the platform has none of its own.
fn coasts() -> bool {
    cfg!(target_os = "linux") && motion::plays()
}

/// Dispatches a scroll step of this layer past its own handler.
fn replay(event: ScrollWheelEvent, window: &mut Window, cx: &mut App) {
    REPLAYING.set(true);
    window.dispatch_event(PlatformInput::ScrollWheel(event), cx);
    REPLAYING.set(false);
}

/// Scrolls by this frame's share of the remaining distance.
fn step(window: &mut Window, cx: &mut App) {
    let now = Instant::now();
    let (delta, modifiers, more) = GLIDE.with_borrow_mut(|glide| {
        let dt = glide.last_frame.map_or(1. / 60., |at| {
            now.duration_since(at).as_secs_f32().min(0.05)
        });
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
    replay(
        ScrollWheelEvent {
            position: window.mouse_position(),
            delta: ScrollDelta::Pixels(delta),
            modifiers,
            touch_phase: TouchPhase::Moved,
        },
        window,
        cx,
    );
    if more {
        window.on_next_frame(step);
    }
}
