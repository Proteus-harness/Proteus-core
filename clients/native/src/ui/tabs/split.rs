//! Dividers between the children of a split: dragging one resizes its
//! neighbors, a double click shares the split evenly and gives fixed panes
//! their default width.

use gpui_kit::component::*;
use gpui_kit::*;

use crate::tabs::{Axis, Size};
use crate::ui::workspace::Workspace;

/// Smallest pane while resizing.
const MIN_WIDTH: f32 = 240.;
const MIN_HEIGHT: f32 = 160.;
const DIVIDER: f32 = 5.;

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
            this.resize_split(
                path.clone(),
                index,
                offset.as_f32(),
                extent.as_f32(),
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
    let theme = cx.theme();
    let (line, hover) = (theme.border, theme.drag_border);
    let id = ElementId::Name(format!("tab-divider-{path:?}-{index}").into());
    let drag = SplitDrag {
        path: path.clone(),
        index,
    };
    let base = match axis {
        Axis::Row => h_flex()
            .w(px(DIVIDER))
            .h_full()
            .justify_center()
            .cursor(CursorStyle::ResizeLeftRight)
            .child(div().w(px(1.)).h_full().bg(line)),
        Axis::Column => v_flex()
            .h(px(DIVIDER))
            .w_full()
            .justify_center()
            .cursor(CursorStyle::ResizeUpDown)
            .child(div().h(px(1.)).w_full().bg(line)),
    };
    base.id(id)
        .flex_none()
        .hover(move |style| style.bg(hover.opacity(0.4)))
        .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
        .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
            if event.click_count() == 2 {
                this.reset_split(&path, cx);
            }
        }))
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
