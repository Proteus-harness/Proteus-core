//! Reasoning effort as a pill-shaped switch: a dot per effort, the filled
//! part up to the chosen one and a round thumb on it. A click or a drag
//! along the pill picks a step; the session hears about it when the button is
//! released. The pill takes the chooser's width, never its labels'.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::*;
use gpui_kit::*;

use super::SelectorBadge;
use crate::ui::{motion, prefs};

/// Height of the pill.
const PILL: f32 = 28.;
/// Room between the thumb and the pill edge.
const INSET: f32 = 3.;
/// Diameter of a step dot.
const DOT: f32 = 6.;
/// Share of the way the thumb covers each frame while it moves.
const EASE: f32 = 0.35;

impl SelectorBadge {
    pub(super) fn pill(&self, count: usize, shown: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let colors = Colors {
            rest: theme.foreground.opacity(0.1),
            fill: theme.primary,
            dot_on: gpui_kit::white().opacity(0.45),
            dot_off: theme.foreground.opacity(0.3),
            thumb: gpui_kit::white(),
        };
        let track = self.track.clone();
        let thumb = self.thumb.clone();
        let this = cx.entity().downgrade();
        let press = this.clone();
        let release = this.clone();
        let paint = canvas(
            move |bounds, _, _| track.set(bounds),
            move |bounds, _, window, _| {
                let at = glide(&thumb, shown as f32, window);
                paint_pill(bounds, count, at, colors, window);
                // The drag follows the pointer even when it leaves the pill.
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble
                        || event.pressed_button != Some(MouseButton::Left)
                    {
                        return;
                    }
                    let _ = this.update(cx, |this, cx| {
                        if this.scrub.is_some() {
                            let step = nearest(this.track.get(), count, event.position.x);
                            this.scrub_to(step, cx);
                        }
                    });
                });
            },
        )
        .absolute()
        .size_full();
        let finish = move |_: &MouseUpEvent, _: &mut Window, cx: &mut App| {
            let _ = release.update(cx, |this, cx| this.finish_scrub(cx));
        };
        div()
            .id("effort-pill")
            .relative()
            .w_full()
            .h(prefs::scaled(PILL))
            .cursor_pointer()
            .child(paint)
            .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                let _ = press.update(cx, |this, cx| {
                    let step = nearest(this.track.get(), count, event.position.x);
                    this.scrub_to(step, cx);
                });
            })
            .on_mouse_up(MouseButton::Left, finish.clone())
            .on_mouse_up_out(MouseButton::Left, finish)
    }
}

#[derive(Clone, Copy)]
struct Colors {
    rest: Hsla,
    fill: Hsla,
    dot_on: Hsla,
    dot_off: Hsla,
    thumb: Hsla,
}

/// Where the thumb is drawn: it slides toward the chosen step over a few
/// frames, or jumps there when motion is reduced.
fn glide(thumb: &Rc<Cell<Option<f32>>>, target: f32, window: &mut Window) -> f32 {
    let at = match thumb.get() {
        Some(at) if motion::moves() && (target - at).abs() > 0.01 => {
            window.request_animation_frame();
            at + (target - at) * EASE
        }
        _ => target,
    };
    thumb.set(Some(at));
    at
}

/// Centre of a step along the pill.
fn center(bounds: Bounds<Pixels>, count: usize, step: f32) -> Pixels {
    let radius = bounds.size.height / 2.;
    let run = bounds.size.width - radius * 2.;
    let share = if count > 1 {
        step / (count - 1) as f32
    } else {
        0.
    };
    bounds.left() + radius + run * share
}

fn nearest(bounds: Bounds<Pixels>, count: usize, x: Pixels) -> usize {
    (0..count)
        .min_by(|a, b| {
            let distance = |step: usize| (center(bounds, count, step as f32) - x).abs();
            f32::from(distance(*a)).total_cmp(&f32::from(distance(*b)))
        })
        .unwrap_or(0)
}

fn circle(center: Point<Pixels>, diameter: Pixels, color: Hsla) -> PaintQuad {
    let radius = diameter / 2.;
    fill(
        Bounds::new(center - point(radius, radius), size(diameter, diameter)),
        color,
    )
    .corner_radii(radius)
}

fn paint_pill(bounds: Bounds<Pixels>, count: usize, at: f32, colors: Colors, window: &mut Window) {
    let radius = bounds.size.height / 2.;
    let middle = bounds.center().y;
    let thumb_x = center(bounds, count, at);
    window.paint_quad(fill(bounds, colors.rest).corner_radii(radius));
    let filled = Bounds::new(
        bounds.origin,
        size(thumb_x + radius - bounds.left(), bounds.size.height),
    );
    window.paint_quad(fill(filled, colors.fill).corner_radii(radius));
    let dot = bounds.size.height * (DOT / PILL);
    for step in 0..count {
        let x = center(bounds, count, step as f32);
        if (x - thumb_x).abs() < radius {
            continue;
        }
        let color = if x < thumb_x {
            colors.dot_on
        } else {
            colors.dot_off
        };
        window.paint_quad(circle(point(x, middle), dot, color));
    }
    let inset = bounds.size.height * (INSET / PILL);
    window.paint_quad(circle(
        point(thumb_x, middle),
        bounds.size.height - inset * 2.,
        colors.thumb,
    ));
}
