//! Shared motion of the client: one set of durations and easings for every
//! animation, so screens move alike.
//!
//! - `enter`: fades and rises of freshly shown blocks, staggered list rows;
//! - `disclosure`: chevrons and blocks that open and close;
//! - `collapse`: a measured horizontal reveal for strips of tabs;
//! - `number`: counters that glide to new values;
//! - `spinner`: the loading spinner;
//! - `ticker`: the shared low-rate clock of looping animations.
//!
//! Animation state lives with the element id, so an id that changes (another
//! page, a reopened section) plays the transition again.
//!
//! The motion preference is read on every call: «уменьшенные» keeps fades and
//! drops movement, «выключены» also sets gpui's reduce-motion flag so every
//! animation renders its final frame.

mod collapse;
mod disclosure;
mod enter;
mod number;
mod slide;
mod spinner;
pub(super) mod ticker;

pub(super) use collapse::Collapse;
pub(super) use disclosure::{chevron, drive, reveal, toggled};
pub(super) use enter::{fade_in_if, rise_in, rise_in_if, stagger_in, switch_in};
pub(super) use number::Number;
pub(super) use slide::{Slide, glide};
pub(super) use spinner::Spinner;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::component::animation::{ease_in_cubic, ease_out_cubic};
use gpui_kit::*;

use crate::preferences::MotionPreference;

/// Hover-sized changes: tabs appearing, small toggles.
pub(super) const FAST: Duration = Duration::from_millis(120);
/// Blocks appearing and panes taking their new places.
pub(super) const BASE: Duration = Duration::from_millis(180);
/// Larger content opening: disclosures, screens.
pub(super) const SLOW: Duration = Duration::from_millis(260);

/// Easing of everything that arrives or moves to a new place.
pub(super) fn ease_enter(t: f32) -> f32 {
    ease_out_cubic(t)
}

/// Easing of everything that leaves.
pub(super) fn ease_exit(t: f32) -> f32 {
    ease_in_cubic(t)
}

static LEVEL: AtomicU8 = AtomicU8::new(0);

pub(super) fn set_level(level: MotionPreference) {
    let value = match level {
        MotionPreference::Full => 0,
        MotionPreference::Reduced => 1,
        MotionPreference::Off => 2,
    };
    LEVEL.store(value, Ordering::Relaxed);
}

/// Whether elements may move (slide, rotate, resize) while animating.
pub(super) fn moves() -> bool {
    LEVEL.load(Ordering::Relaxed) == 0
}

/// Whether anything animates at all.
pub(super) fn plays() -> bool {
    LEVEL.load(Ordering::Relaxed) < 2
}

fn shift(distance: f32) -> Pixels {
    px(if moves() { distance } else { 0. })
}

/// A transition started at a moment; samples its progress over time.
#[derive(Clone, Copy, Debug)]
pub(super) struct Tween {
    at: Instant,
    duration: Duration,
}

impl Tween {
    pub(super) fn start(duration: Duration) -> Self {
        Self {
            at: Instant::now(),
            duration,
        }
    }

    /// Linear progress in `0..=1`.
    pub(super) fn linear(&self, now: Instant) -> f32 {
        let duration = self.duration.as_secs_f32();
        if duration <= 0. {
            return 1.;
        }
        (now.saturating_duration_since(self.at).as_secs_f32() / duration).min(1.)
    }

    pub(super) fn running(&self, now: Instant) -> bool {
        self.age(now) < self.duration
    }

    /// Time since the transition started.
    pub(super) fn age(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.at)
    }
}

/// Keys that changed a moment ago: lets list rows animate only right after
/// they appeared, not every time they scroll back into view.
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
}
