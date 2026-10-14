//! Inertia of touchpad scrolling. Wayland tells when the fingers leave the
//! pad, but gpui drops that event, so a pause in pixel scrolling stands in
//! for it: content flicked fast enough keeps gliding with the speed of the
//! fingers and slows down as on ice. A new scroll or a click stops it.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gpui_kit::*;

/// Silence after which the fingers count as lifted: a few missed touchpad
/// reports, which come every 7-12 ms while the fingers move.
const PAUSE: Duration = Duration::from_millis(30);
/// The last part of the gesture that gives the speed of the flick.
const WINDOW: Duration = Duration::from_millis(60);
/// Slower gestures stop where the fingers left them.
const MIN_SPEED: f32 = 300.;
const MAX_SPEED: f32 = 8000.;
const STOP_SPEED: f32 = 20.;
/// Time constant of the friction: the speed shrinks by ~63% per τ, and a
/// glide covers about speed × τ.
const FRICTION_SECONDS: f32 = 0.4;

#[derive(Default, PartialEq)]
enum State {
    #[default]
    Idle,
    /// Fingers move; waiting for the pause that ends the gesture.
    Tracking,
    Gliding,
}

#[derive(Default)]
struct Fling {
    state: State,
    samples: VecDeque<(Instant, Point<Pixels>)>,
    position: Point<Pixels>,
    modifiers: Modifiers,
    /// Pixels per second.
    velocity: Point<f32>,
    last_frame: Option<Instant>,
    /// A frame callback is scheduled; `stop` alone does not cancel it.
    looping: bool,
}

thread_local! {
    static FLING: RefCell<Fling> = RefCell::default();
}

/// Records one step of a touchpad gesture; a glide in progress stops.
pub(super) fn track(
    position: Point<Pixels>,
    delta: Point<Pixels>,
    modifiers: Modifiers,
    window: &mut Window,
) {
    let now = Instant::now();
    let start = FLING.with_borrow_mut(|fling| {
        fling.samples.push_back((now, delta));
        while fling
            .samples
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > WINDOW)
        {
            fling.samples.pop_front();
        }
        fling.position = position;
        fling.modifiers = modifiers;
        fling.state = State::Tracking;
        !std::mem::replace(&mut fling.looping, true)
    });
    if start {
        window.on_next_frame(tick);
    }
}

pub(super) fn stop() {
    FLING.with_borrow_mut(|fling| {
        fling.state = State::Idle;
        fling.samples.clear();
    });
}

fn tick(window: &mut Window, cx: &mut App) {
    let now = Instant::now();
    let step = FLING.with_borrow_mut(|fling| match fling.state {
        State::Idle => None,
        State::Tracking => {
            let Some(&(last, _)) = fling.samples.back() else {
                fling.state = State::Idle;
                return None;
            };
            if now.duration_since(last) < PAUSE {
                return None;
            }
            let velocity = speed(&fling.samples);
            fling.samples.clear();
            let size = velocity.x.hypot(velocity.y);
            if size < MIN_SPEED {
                fling.state = State::Idle;
                return None;
            }
            let scale = size.min(MAX_SPEED) / size;
            fling.velocity = point(velocity.x * scale, velocity.y * scale);
            // Catching up the pause in one frame would read as a jump.
            fling.last_frame = None;
            fling.state = State::Gliding;
            Some(advance(fling, now))
        }
        State::Gliding => Some(advance(fling, now)),
    });
    if let Some(delta) = step {
        let (position, modifiers) = FLING.with_borrow(|fling| (fling.position, fling.modifiers));
        super::replay(
            ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(delta),
                modifiers,
                touch_phase: TouchPhase::Moved,
            },
            window,
            cx,
        );
    }
    let more = FLING.with_borrow_mut(|fling| {
        fling.looping = fling.state != State::Idle;
        fling.looping
    });
    if more {
        window.on_next_frame(tick);
    }
}

/// Moves the glide to `now`, slowing it down; the glide ends once slow.
fn advance(fling: &mut Fling, now: Instant) -> Point<Pixels> {
    let dt = fling
        .last_frame
        .map_or(1. / 60., |at| now.duration_since(at).as_secs_f32().min(0.1));
    fling.last_frame = Some(now);
    let decay = (-dt / FRICTION_SECONDS).exp();
    let travel = FRICTION_SECONDS * (1. - decay);
    let delta = point(px(fling.velocity.x * travel), px(fling.velocity.y * travel));
    fling.velocity = point(fling.velocity.x * decay, fling.velocity.y * decay);
    if fling.velocity.x.hypot(fling.velocity.y) < STOP_SPEED {
        fling.state = State::Idle;
    }
    delta
}

/// Average speed over the samples: each step covers the time since the
/// previous one, so the first only marks the start.
fn speed(samples: &VecDeque<(Instant, Point<Pixels>)>) -> Point<f32> {
    let (Some((first, _)), Some((last, _))) = (samples.front(), samples.back()) else {
        return Point::default();
    };
    let span = last.duration_since(*first).as_secs_f32();
    if samples.len() < 2 || span < 0.008 {
        return Point::default();
    }
    let total = samples
        .iter()
        .skip(1)
        .fold(Point::<Pixels>::default(), |sum, (_, delta)| sum + *delta);
    point(f32::from(total.x) / span, f32::from(total.y) / span)
}
