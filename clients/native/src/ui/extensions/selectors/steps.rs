//! Reasoning effort as a stepped track: a tick per effort, the filled part up
//! to the chosen one and a square thumb on it, cornered like the other
//! controls. A click or a drag along the track picks a step; the session
//! hears about it when the button is released. The track takes the chooser's
//! width, never its labels'.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::*;
use gpui_kit::*;

use super::SelectorBadge;
use crate::ui::{motion, prefs};

/// Height of the track.
const PILL: f32 = 28.;
/// Room between the thumb and the track edge.
const INSET: f32 = 3.;
/// Width and height of a step tick.
const TICK: (f32, f32) = (2., 10.);
/// Share of the way the thumb covers each frame while it moves.
const EASE: f32 = 0.35;

impl SelectorBadge {
    pub(super) fn pill(&self, count: usize, shown: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let corner = theme.radius;
        let colors = Colors {
            rest: theme.foreground.opacity(0.1),
            fill: theme.primary,
            tick_on: gpui_kit::white().opacity(0.45),
            tick_off: theme.foreground.opacity(0.3),
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
                paint_pill(bounds, count, at, colors, corner, window);
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
    tick_on: Hsla,
    tick_off: Hsla,
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

/// Centre of a step along the track; the end steps keep the thumb inside.
fn center(bounds: Bounds<Pixels>, count: usize, step: f32) -> Pixels {
    let edge = bounds.size.height / 2.;
    let run = bounds.size.width - edge * 2.;
    let share = if count > 1 {
        step / (count - 1) as f32
    } else {
        0.
    };
    bounds.left() + edge + run * share
}

fn nearest(bounds: Bounds<Pixels>, count: usize, x: Pixels) -> usize {
    (0..count)
        .min_by(|a, b| {
            let distance = |step: usize| (center(bounds, count, step as f32) - x).abs();
            f32::from(distance(*a)).total_cmp(&f32::from(distance(*b)))
        })
        .unwrap_or(0)
}

fn centered(center: Point<Pixels>, extent: gpui_kit::Size<Pixels>, color: Hsla) -> PaintQuad {
    fill(
        Bounds::new(
            center - point(extent.width / 2., extent.height / 2.),
            extent,
        ),
        color,
    )
}

fn paint_pill(
    bounds: Bounds<Pixels>,
    count: usize,
    at: f32,
    colors: Colors,
    corner: Pixels,
    window: &mut Window,
) {
    let edge = bounds.size.height / 2.;
    let middle = bounds.center().y;
    let thumb_x = center(bounds, count, at);
    window.paint_quad(fill(bounds, colors.rest).corner_radii(corner));
    let filled = Bounds::new(
        bounds.origin,
        size(thumb_x + edge - bounds.left(), bounds.size.height),
    );
    window.paint_quad(fill(filled, colors.fill).corner_radii(corner));
    let unit = bounds.size.height / PILL;
    let tick = size(unit * TICK.0, unit * TICK.1);
    for step in 0..count {
        let x = center(bounds, count, step as f32);
        if (x - thumb_x).abs() < edge {
            continue;
        }
        let color = if x < thumb_x {
            colors.tick_on
        } else {
            colors.tick_off
        };
        window.paint_quad(centered(point(x, middle), tick, color).corner_radii(tick.width / 2.));
    }
    let inset = unit * INSET;
    let side = bounds.size.height - inset * 2.;
    window.paint_quad(
        centered(point(thumb_x, middle), size(side, side), colors.thumb)
            .corner_radii((corner - inset).max(px(1.))),
    );
}
