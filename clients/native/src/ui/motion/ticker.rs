//! One clock for the looping animations — spinners and the logo ripple. Each
//! of them asking for frames on its own cadence would add up to a redraw on
//! nearly every display frame; on a shared tick they all advance in the same
//! frame, at a rate that still reads as motion.

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui_kit::*;

/// Ticks per second while the window is active, and while it is not.
const ACTIVE_FPS: f32 = 24.;
const IDLE_FPS: f32 = 8.;

thread_local! {
    static PENDING: Cell<bool> = const { Cell::new(false) };
    static WAITING: RefCell<Vec<EntityId>> = const { RefCell::new(Vec::new()) };
}

/// Where a loop of `period` is now, from 0 to 1; loops started at different
/// times stay in step.
pub(in crate::ui) fn phase(period: Duration) -> f32 {
    static STARTED: OnceLock<Instant> = OnceLock::new();
    let elapsed = STARTED.get_or_init(Instant::now).elapsed().as_secs_f32();
    (elapsed / period.as_secs_f32()).fract()
}

/// Redraws the view being drawn on the next tick.
pub(in crate::ui) fn next_tick(window: &mut Window, cx: &mut App) {
    let view = window.current_view();
    WAITING.with_borrow_mut(|waiting| {
        if !waiting.contains(&view) {
            waiting.push(view);
        }
    });
    if PENDING.replace(true) {
        return;
    }
    let fps = if window.is_window_active() {
        ACTIVE_FPS
    } else {
        IDLE_FPS
    };
    window
        .spawn(cx, async move |cx| {
            cx.background_executor()
                .timer(Duration::from_secs_f32(1. / fps))
                .await;
            PENDING.set(false);
            let views = WAITING.take();
            cx.update(|_, cx| {
                for view in views {
                    cx.notify(view);
                }
            })
            .ok();
        })
        .detach();
}
