//! Keeps the control the keyboard reached in the visible part of the page:
//! each card cell tracks a focus handle of its own, and the cell holding the
//! focused control scrolls into view once per focus change.

use std::cell::{Cell, RefCell};

use gpui_kit::base::ElementExt as _;
use gpui_kit::*;

/// Room kept between a revealed cell and the edge of the page.
const MARGIN: f32 = 24.;

thread_local! {
    /// Cell handles, reused by position from frame to frame.
    static CELLS: RefCell<Vec<FocusHandle>> = const { RefCell::new(Vec::new()) };
    static NEXT: Cell<usize> = const { Cell::new(0) };
    /// The scrolled page being built, if any.
    static PAGE: RefCell<Option<ScrollHandle>> = const { RefCell::new(None) };
    /// The focus already brought into view.
    static SHOWN: RefCell<Option<FocusHandle>> = const { RefCell::new(None) };
}

/// Starts a page: cells built until `end` scroll `scroll`.
pub(super) fn begin(scroll: ScrollHandle) {
    NEXT.set(0);
    PAGE.replace(Some(scroll));
}

pub(super) fn end() {
    PAGE.replace(None);
}

/// A card cell; outside a page it is a plain container.
pub(super) fn cell(cx: &App) -> Div {
    let Some(scroll) = PAGE.with_borrow(Clone::clone) else {
        return div();
    };
    let ix = NEXT.replace(NEXT.get() + 1);
    let handle = CELLS.with_borrow_mut(|cells| {
        while cells.len() <= ix {
            cells.push(cx.focus_handle());
        }
        cells[ix].clone()
    });
    let probe = handle.clone();
    div()
        .track_focus(&handle)
        .on_prepaint(move |bounds, window, cx| reveal(&probe, &scroll, bounds, window, cx))
}

fn reveal(
    cell: &FocusHandle,
    scroll: &ScrollHandle,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    if !window.last_input_was_keyboard() {
        return;
    }
    let Some(focused) = window.focused(cx) else {
        return;
    };
    // Nested cells come first, so the closest one moves the page.
    if SHOWN.with_borrow(|shown| shown.as_ref() == Some(&focused))
        || !cell.contains_focused(window, cx)
    {
        return;
    }
    SHOWN.replace(Some(focused));
    let view = scroll.bounds();
    let (top, bottom) = (view.top() + px(MARGIN), view.bottom() - px(MARGIN));
    let shift = if bounds.top() < top || bounds.size.height > bottom - top {
        top - bounds.top()
    } else if bounds.bottom() > bottom {
        bottom - bounds.bottom()
    } else {
        return;
    };
    let mut offset = scroll.offset();
    offset.y = (offset.y + shift).clamp(-scroll.max_offset().y, px(0.));
    scroll.set_offset(offset);
    window.request_animation_frame();
}
