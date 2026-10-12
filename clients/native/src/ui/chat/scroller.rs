//! The chat's virtual list with its own tail following.
//!
//! gpui's `FollowMode::Tail` snaps to the end on every layout. Here the list
//! stays in normal mode and the scroller glides toward the end itself, so a
//! streaming answer scrolls smoothly and following can be switched off.
//!
//! Following stops when the user moves up (the offset drops while content
//! remains below) and resumes once the view reaches the end again.

use std::ops::Range;
use std::time::Instant;

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollableMask};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::motion;

const OVERDRAW: Pixels = px(400.);
/// Time constant of the glide: the remaining distance shrinks by ~63% per τ.
const GLIDE_SECONDS: f32 = 0.08;
/// Distances longer than this many viewports jump instead of gliding.
const JUMP_VIEWPORTS: f32 = 2.;
const SETTLED: Pixels = px(0.5);

pub(super) struct ChatScroller {
    list: ListState,
    following: bool,
    autoscroll: bool,
    /// Offset seen on the previous frame, to tell the user's scrolling apart.
    last_offset: Option<Pixels>,
    last_frame: Option<Instant>,
    /// Content changed since the last frame; the new size is known only after
    /// the next layout.
    dirty: bool,
    /// Width of the viewport on the last layout.
    width: Option<Pixels>,
}

impl ChatScroller {
    pub(super) fn new(autoscroll: bool, cx: &mut Context<Self>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, OVERDRAW);
        let weak = cx.weak_entity();
        list.set_scroll_handler(move |_, _, cx| {
            let weak = weak.clone();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |_, cx| cx.notify());
            });
        });
        Self {
            list,
            following: true,
            autoscroll,
            last_offset: None,
            last_frame: None,
            dirty: false,
            width: None,
        }
    }

    pub(super) fn set_autoscroll(&mut self, autoscroll: bool, cx: &mut Context<Self>) {
        if self.autoscroll == autoscroll {
            return;
        }
        self.autoscroll = autoscroll;
        self.following = autoscroll && self.gap() <= SETTLED;
        cx.notify();
    }

    pub(super) fn splice(&mut self, range: Range<usize>, count: usize, cx: &mut Context<Self>) {
        if range.start > range.end || range.end > self.list.item_count() {
            return;
        }
        self.list.splice(range, count);
        self.changed(cx);
    }

    pub(super) fn append(&mut self, count: usize, cx: &mut Context<Self>) {
        let end = self.list.item_count();
        self.splice(end..end, count, cx);
    }

    pub(super) fn remeasure(&mut self, cx: &mut Context<Self>) {
        self.list.remeasure();
        self.changed(cx);
    }

    pub(super) fn remeasure_items(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        if range.start > range.end || range.end > self.list.item_count() {
            return;
        }
        self.list.remeasure_items(range);
        self.changed(cx);
    }

    /// Bring the latest row into view; follows again when autoscroll is on.
    pub(super) fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.following = true;
        self.dirty = true;
        cx.notify();
    }

    /// A new width re-measures every row and keeps the top one in place, so
    /// the end would drop out of view; a following list stays at its end.
    /// Called before the list lays out at `width`.
    fn keep_end(&mut self, width: Pixels) {
        let resized = self.width.replace(width).is_some_and(|last| last != width);
        if resized && self.following {
            self.list.scroll_to_end();
            // Rows above are unmeasured now, so the offset reads lower
            // without the user scrolling.
            self.last_offset = None;
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        cx.notify();
    }

    /// Pixels between the current position and the end of the content.
    fn gap(&self) -> Pixels {
        let max = self.list.max_offset_for_scrollbar().y;
        let offset = -self.list.scroll_px_offset_for_scrollbar().y;
        (max - offset).max(px(0.))
    }

    fn scrolled_up(&self) -> bool {
        !self.following && self.gap() > SETTLED
    }

    /// Advance following by one frame; returns whether another frame is needed.
    fn step(&mut self, now: Instant) -> bool {
        let offset = -self.list.scroll_px_offset_for_scrollbar().y;
        let gap = self.gap();
        let dt = self
            .last_frame
            .map_or(1. / 60., |at| now.duration_since(at).as_secs_f32().min(0.1));
        self.last_frame = Some(now);

        // Content growth leaves the offset alone and an item collapsing above
        // lowers both; only the user moves up while room stays below.
        if let Some(last) = self.last_offset
            && offset < last - px(1.)
            && gap > SETTLED
        {
            self.following = false;
        }
        if !self.following
            && gap <= SETTLED
            && self.autoscroll
            && !self.list.is_scrollbar_dragging()
        {
            self.following = true;
        }
        let dirty = std::mem::take(&mut self.dirty);

        let mut running = dirty;
        if self.following && gap > SETTLED {
            let viewport = self.list.viewport_bounds().size.height;
            if !motion::plays() || gap > viewport * JUMP_VIEWPORTS {
                self.list.scroll_to_end();
                // The end anchor reads past the content until laid out.
                self.last_frame = Some(now);
                self.last_offset = None;
                return true;
            } else {
                let share = 1. - (-dt / GLIDE_SECONDS).exp();
                self.list.scroll_by((gap * share).max(px(1.)).min(gap));
            }
            running = true;
        }
        if self.following && !self.autoscroll && !dirty && gap <= SETTLED {
            // A jump without autoscroll ends once the end is reached.
            self.following = false;
        }
        self.last_offset = Some(-self.list.scroll_px_offset_for_scrollbar().y);
        if !running {
            self.last_frame = None;
        }
        running
    }
}

type RenderRow = Box<dyn FnMut(usize, &mut Window, &mut App) -> AnyElement>;

/// The list element; rows come from the caller's renderer.
#[derive(IntoElement)]
pub(super) struct ChatList {
    id: ElementId,
    state: Entity<ChatScroller>,
    render_row: RenderRow,
}

impl ChatList {
    pub(super) fn new(
        id: impl Into<ElementId>,
        state: Entity<ChatScroller>,
        render_row: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            state,
            render_row: Box::new(render_row),
        }
    }
}

impl RenderOnce for ChatList {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.state.update(cx, |state, _| state.step(Instant::now())) {
            window.request_animation_frame();
        }
        let (list_state, scrolled_up) = {
            let state = self.state.read(cx);
            (state.list.clone(), state.scrolled_up())
        };
        let mut render_row = self.render_row;
        let rows = list(list_state.clone(), move |ix, window, cx| {
            div()
                .w_full()
                .min_w_0()
                .child(render_row(ix, window, cx))
                .into_any_element()
        })
        .size_full()
        .min_h_0()
        .py_2();
        let state = self.state.clone();
        let resized = self.state.clone();
        div()
            .id(self.id.clone())
            .relative()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .child(
                div()
                    .id((self.id.clone(), "viewport"))
                    .size_full()
                    .min_h_0()
                    .min_w_0()
                    // Ahead of the rows, so it runs before the list lays out.
                    .on_prepaint(move |bounds, _, cx| {
                        resized.update(cx, |state, _| state.keep_end(bounds.size.width))
                    })
                    .child(rows)
                    .vertical_scrollbar(&list_state),
            )
            .child(ScrollableMask::new(Axis::Vertical, &list_state).id(self.id.clone()))
            .when(scrolled_up, |this| {
                this.child(
                    h_flex()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_3()
                        .justify_center()
                        .child(motion::rise_in(
                            kit::button((self.id.clone(), "jump"))
                                .secondary()
                                .icon(IconName::ArrowDown)
                                .hint("К последним сообщениям", cx)
                                .rounded(cx.theme().radius_full())
                                .border_1()
                                .border_color(cx.theme().border)
                                .bg(cx.theme().background)
                                .on_click(move |_, _, cx| {
                                    state.update(cx, |state, cx| state.scroll_to_end(cx));
                                }),
                            (self.id, "jump-enter"),
                        )),
                )
            })
    }
}
