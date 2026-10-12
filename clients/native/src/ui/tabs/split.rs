//! Dividers between the children of a split: dragging one resizes its
//! neighbors, a double click shares the split evenly between the panes
//! that are not fixed.

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::tabs::{Axis, Size};
use crate::ui::workspace::Workspace;
use crate::ui::{drag, prefs};

/// Smallest pane while resizing.
const MIN_WIDTH: f32 = 240.;
const MIN_HEIGHT: f32 = 160.;
/// The line between panes, and the strip around it that can be dragged.
pub(super) const DIVIDER: f32 = 1.;
const GRIP: f32 = 7.;
/// The line over a hovered or dragged divider.
const MARK: f32 = 3.;

/// Payload of a divider while it is dragged: the split and the child the
/// divider follows.
#[derive(Clone)]
pub(in crate::ui) struct SplitDrag {
    path: Vec<usize>,
    index: usize,
}

impl Render for SplitDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Sizes of a split's children while its divider is dragged; stored when
/// released.
pub(in crate::ui) struct SplitResize {
    pub path: Vec<usize>,
    pub sizes: Vec<Size>,
}

/// Follows a dragged divider of the split at `path`; goes on the split's
/// own element, whose bounds the position is measured against.
pub(super) fn track_resize<E: InteractiveElement>(
    element: E,
    path: Vec<usize>,
    axis: Axis,
    cx: &Context<Workspace>,
) -> E {
    element.on_drag_move::<SplitDrag>(cx.listener(
        move |this, event: &DragMoveEvent<SplitDrag>, _, cx| {
            let drag = event.drag(cx);
            if drag.path != path {
                return;
            }
            let index = drag.index;
            let (bounds, position) = (event.bounds, event.event.position);
            let (offset, extent, min) = match axis {
                Axis::Row => (position.x - bounds.left(), bounds.size.width, MIN_WIDTH),
                Axis::Column => (position.y - bounds.top(), bounds.size.height, MIN_HEIGHT),
            };
            let scale = prefs::scale(cx);
            this.resize_split(
                path.clone(),
                index,
                offset.as_f32() / scale,
                extent.as_f32() / scale,
                min,
                cx,
            );
        },
    ))
}

/// The divider after child `index` of the split at `path`.
pub(super) fn divider(
    path: Vec<usize>,
    index: usize,
    axis: Axis,
    cx: &Context<Workspace>,
) -> impl IntoElement {
    let name = SharedString::from(format!("tab-divider-{path:?}-{index}"));
    let drag = SplitDrag {
        path: path.clone(),
        index,
    };
    let grip = divider_grip(name.clone(), axis, cx)
        .on_drag(drag, move |drag, _, _, cx| {
            drag::start(name.clone(), cx);
            cx.new(|_| drag.clone())
        })
        .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
            if event.click_count() == 2 {
                this.reset_split(&path, cx);
            }
        }));
    divider_line(axis, grip, cx)
}

/// A divider: a hairline in the layout under `grip`. The grip is drawn
/// after everything else, so the next neighbour does not cover it.
pub(in crate::ui) fn divider_line(axis: Axis, grip: Stateful<Div>, cx: &App) -> Div {
    let base = match axis {
        Axis::Row => div().w(prefs::scaled(DIVIDER)).h_full(),
        Axis::Column => div().h(prefs::scaled(DIVIDER)).w_full(),
    };
    base.relative()
        .flex_none()
        .bg(cx.theme().border)
        .child(deferred(grip))
}

/// The strip over a divider wider than its hairline that takes the
/// pointer, named `name` as a drag source; marked by a line while hovered
/// or dragged.
pub(in crate::ui) fn divider_grip(name: SharedString, axis: Axis, cx: &App) -> Stateful<Div> {
    let accent = cx.theme().drag_border;
    let held = drag::is_source(&name, cx);
    let reach = px((GRIP - DIVIDER) / 2. * prefs::scale(cx));
    let grip = div()
        .id(ElementId::Name(name.clone()))
        .group(name.clone())
        .absolute()
        .flex()
        .justify_center();
    let mark = div()
        .when(held, |mark| mark.bg(accent))
        .group_hover(name, move |style| style.bg(accent));
    match axis {
        Axis::Row => grip
            .top_0()
            .bottom_0()
            .left(-reach)
            .right(-reach)
            .cursor(CursorStyle::ResizeLeftRight)
            .child(mark.w(prefs::scaled(MARK)).h_full()),
        Axis::Column => grip
            .flex_col()
            .left_0()
            .right_0()
            .top(-reach)
            .bottom(-reach)
            .cursor(CursorStyle::ResizeUpDown)
            .child(mark.h(prefs::scaled(MARK)).w_full()),
    }
}

impl Workspace {
    /// Puts the divider after child `index` at `offset` pixels into the
    /// split, which is `extent` long including its dividers.
    fn resize_split(
        &mut self,
        path: Vec<usize>,
        index: usize,
        offset: f32,
        extent: f32,
        min: f32,
        cx: &mut Context<Self>,
    ) {
        let mut layout = self.tab_layout(cx);
        let Some(children) = layout.sizes(&path).map(|sizes| sizes.len()) else {
            return;
        };
        let content = extent - (children - 1) as f32 * DIVIDER;
        if content <= 0. {
            return;
        }
        let at = offset - (index as f32 + 0.5) * DIVIDER;
        layout.resize(&path, index, at, content, min);
        let Some(sizes) = layout.sizes(&path) else {
            return;
        };
        self.split_drag = Some(SplitResize { path, sizes });
        cx.notify();
    }

    /// Stores the size of the split whose divider was released.
    pub(in crate::ui) fn finish_resize(&mut self, cx: &mut Context<Self>) {
        if let Some(resize) = self.split_drag.take() {
            self.update_tabs(cx, |tabs| tabs.set_sizes(&resize.path, &resize.sizes));
        }
    }

    fn reset_split(&mut self, path: &[usize], cx: &mut Context<Self>) {
        self.split_drag = None;
        self.update_tabs(cx, |tabs| tabs.reset(path));
    }
}
