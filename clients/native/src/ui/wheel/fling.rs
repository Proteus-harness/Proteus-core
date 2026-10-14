//! Inertia of touchpad scrolling. Wayland tells when the fingers leave the
//! pad, but gpui drops that event, so a pause in pixel scrolling stands in
//! for it: content flicked fast enough keeps gliding with the speed the
//! fingers had at the end and slows down as on ice. A new scroll or a click
//! stops it.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use gpui_kit::*;

/// Silence after which the fingers count as lifted: a few missed touchpad
/// reports, which come every 7-12 ms while the fingers move.
const PAUSE: Duration = Duration::from_millis(30);
/// Usual interval between touchpad reports; closer ones were bunched on the
/// way to the window and are spread over it.
const REPORT: Duration = Duration::from_millis(8);
/// How fast the speed estimate follows the fingers. The last ~20 ms weigh
/// most, so fingers that slow down before lifting leave no glide.
const FOLLOW_SECONDS: f32 = 0.012;
/// Slower gestures stop where the fingers left them.
const MIN_SPEED: f32 = 500.;
const MAX_SPEED: f32 = 8000.;
const STOP_SPEED: f32 = 40.;
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
    last_report: Option<Instant>,
    reports: usize,
    position: Point<Pixels>,
    modifiers: Modifiers,
    /// Pixels per second: of the fingers while tracking, of the glide after.
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
        let since = fling
            .last_report
            .filter(|_| fling.state == State::Tracking)
            .map(|last| now.duration_since(last));
        let dt = since.unwrap_or(REPORT).max(REPORT).as_secs_f32();
        let speed = point(f32::from(delta.x) / dt, f32::from(delta.y) / dt);
        if since.is_some() {
            let follow = 1. - (-dt / FOLLOW_SECONDS).exp();
            fling.velocity = point(
                fling.velocity.x + (speed.x - fling.velocity.x) * follow,
                fling.velocity.y + (speed.y - fling.velocity.y) * follow,
            );
            fling.reports += 1;
        } else {
            fling.velocity = speed;
            fling.reports = 1;
        }
        fling.last_report = Some(now);
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
    FLING.with_borrow_mut(|fling| fling.state = State::Idle);
}

fn tick(window: &mut Window, cx: &mut App) {
    let now = Instant::now();
    let step = FLING.with_borrow_mut(|fling| match fling.state {
        State::Idle => None,
        State::Tracking => {
            let last = fling.last_report?;
            if now.duration_since(last) < PAUSE {
                return None;
            }
            let velocity = fling.velocity;
            let size = velocity.x.hypot(velocity.y);
            // A single report is a nudge rather than a flick.
            if fling.reports < 2 || size < MIN_SPEED {
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
