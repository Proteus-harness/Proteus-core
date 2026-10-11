//! The Proteus pixel logo drawn cell by cell; while the agent works its
//! columns ripple.

use std::f32::consts::TAU;
use std::sync::OnceLock;
use std::time::Instant;

use gpui_kit::*;

use crate::ui::{motion, prefs};

const COLUMNS: f32 = 12.;
const ROWS: f32 = 10.;
/// Seconds for the ripple to cross the logo.
const PERIOD: f32 = 1.4;

/// Runs of one color: column, row, width in cells.
const CELLS: [(u8, u8, u8, u32); 33] = [
    (0, 0, 2, 0xed1c24),
    (2, 0, 2, 0xffa3b1),
    (4, 0, 3, 0x6f3198),
    (7, 0, 3, 0xff7e00),
    (10, 0, 2, 0x2f3699),
    (0, 1, 3, 0xed1c24),
    (3, 1, 2, 0xffa3b1),
    (5, 1, 3, 0x6f3198),
    (8, 1, 3, 0xff7e00),
    (11, 1, 1, 0x2f3699),
    (2, 2, 1, 0xed1c24),
    (3, 2, 1, 0xffa3b1),
    (8, 2, 1, 0xff7e00),
    (9, 2, 1, 0x99d9ea),
    (2, 3, 1, 0xed1c24),
    (3, 3, 1, 0xffa3b1),
    (8, 3, 2, 0x99d9ea),
    (2, 4, 2, 0xffa3b1),
    (8, 4, 2, 0x99d9ea),
    (2, 5, 2, 0x22b14c),
    (8, 5, 2, 0x99d9ea),
    (2, 6, 2, 0x22b14c),
    (8, 6, 1, 0xffc20e),
    (9, 6, 1, 0x99d9ea),
    (2, 7, 1, 0x22b14c),
    (3, 7, 1, 0x990030),
    (8, 7, 1, 0xffc20e),
    (9, 7, 1, 0x99d9ea),
    (2, 8, 1, 0x22b14c),
    (3, 8, 1, 0x990030),
    (8, 8, 2, 0xffc20e),
    (2, 9, 2, 0x990030),
    (8, 9, 2, 0xffc20e),
];

/// The logo `cell` wide per pixel; `busy` sets it rippling.
pub(super) fn logo(cell: f32, busy: bool) -> impl IntoElement {
    let ripple = busy && motion::plays();
    let lift = motion::moves();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let unit = bounds.size.width / COLUMNS;
            let phase = if ripple {
                started().elapsed().as_secs_f32() / PERIOD
            } else {
                0.
            };
            for &(x, y, width, color) in &CELLS {
                let color = rgb(color);
                if !ripple {
                    let origin = bounds.origin + point(unit * f32::from(x), unit * f32::from(y));
                    window.paint_quad(fill(
                        Bounds::new(origin, size(unit * f32::from(width), unit)),
                        color,
                    ));
                    continue;
                }
                for column in x..x + width {
                    let wave = (TAU * (phase - f32::from(column) / COLUMNS)).sin().max(0.);
                    let (offset, alpha) = if lift { (wave, 1.) } else { (0., 1. - 0.45 * wave) };
                    let origin = bounds.origin
                        + point(unit * f32::from(column), unit * (f32::from(y) - offset));
                    window.paint_quad(fill(
                        Bounds::new(origin, size(unit, unit)),
                        Rgba { a: alpha, ..color },
                    ));
                }
            }
            if ripple {
                window.request_animation_frame();
            }
        },
    )
    .flex_none()
    .w(prefs::scaled(cell * COLUMNS))
    .h(prefs::scaled(cell * ROWS))
}

fn started() -> Instant {
    static STARTED: OnceLock<Instant> = OnceLock::new();
    *STARTED.get_or_init(Instant::now)
}
