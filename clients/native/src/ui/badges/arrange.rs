//! Live rearranging while an icon is dragged: the row it hovers parts where
//! the icon would land, and its old row closes up behind it.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui_kit::*;

use crate::preferences::BadgePlace;
use crate::ui::{drag, extensions, prefs};

/// Where the dragged icon would land.
#[derive(Clone, Copy, PartialEq)]
enum Target {
    /// The drag just started: the icon still holds its own place.
    Start,
    Outside,
    At(BadgePlace, usize),
}

struct Arrange {
    dragging: Option<&'static str>,
    /// Bumped whenever a drag starts or ends, so the rows are drawn afresh
    /// instead of gliding between the dragged and the settled layout.
    generation: u64,
    target: Target,
    /// Last drawn bounds of every icon, for hit testing and the gap width.
    bounds: HashMap<&'static str, Bounds<Pixels>>,
}

thread_local! {
    static STATE: RefCell<Arrange> = RefCell::new(Arrange {
        dragging: None,
        generation: 0,
        target: Target::Outside,
        bounds: HashMap::new(),
    });
}

/// How one row is drawn this frame.
pub(super) struct Frame {
    /// The icon being dragged, if any icon is.
    pub dragging: Option<&'static str>,
    /// Gap among the other icons of this row that stays open.
    pub open_gap: Option<usize>,
    pub generation: u64,
    /// Width of the dragged icon, the room a gap opens to.
    pub gap_width: Pixels,
}

pub(super) fn frame(place: BadgePlace, ids: &[&'static str], cx: &App) -> Frame {
    let dragging = drag::current(cx)
        .and_then(|key| key.strip_prefix("badge:"))
        .and_then(|id| extensions::badge_ids().into_iter().find(|known| *known == id));
    STATE.with_borrow_mut(|state| {
        if state.dragging != dragging {
            state.dragging = dragging;
            state.generation += 1;
            state.target = Target::Start;
        }
        let open_gap = match state.target {
            Target::At(at, index) if at == place => Some(index),
            Target::Start => dragging.and_then(|id| ids.iter().position(|known| *known == id)),
            _ => None,
        };
        let gap_width = dragging
            .and_then(|id| state.bounds.get(id))
            .map_or(px(28. * prefs::scale(cx)), |bounds| bounds.size.width);
        Frame {
            dragging,
            open_gap,
            generation: state.generation,
            gap_width,
        }
    })
}

pub(super) fn record(id: &'static str, bounds: Bounds<Pixels>) {
    STATE.with_borrow_mut(|state| {
        state.bounds.insert(id, bounds);
    });
}

/// Follows the pointer over a row: inside it the gap goes in front of the
/// first icon whose middle lies to the right; leaving it closes the gap.
pub(super) fn hover(
    place: BadgePlace,
    others: &[&'static str],
    holds_source: bool,
    row: Bounds<Pixels>,
    position: Point<Pixels>,
    window: &mut Window,
) {
    STATE.with_borrow_mut(|state| {
        let next = if row.contains(&position) {
            let index = others
                .iter()
                .filter(|id| {
                    state
                        .bounds
                        .get(*id)
                        .is_some_and(|bounds| bounds.center().x < position.x)
                })
                .count();
            Target::At(place, index)
        } else {
            match state.target {
                Target::At(at, _) if at == place => Target::Outside,
                Target::Start if holds_source => Target::Outside,
                _ => return,
            }
        };
        if state.target != next {
            state.target = next;
            window.refresh();
        }
    });
}

/// Index among the other icons of `place` where a drop lands.
pub(super) fn drop_index(place: BadgePlace) -> Option<usize> {
    STATE.with_borrow(|state| match state.target {
        Target::At(at, index) if at == place => Some(index),
        _ => None,
    })
}
