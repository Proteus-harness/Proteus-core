//! Dropping a dragged tab onto a pane: the middle moves it into the pane,
//! the edges split the pane. The part that will be taken is highlighted.

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::tab::DraggedTab;
use crate::tabs::{self, Edge, PaneId};
use crate::ui::workspace::Workspace;

/// Where a dragged tab would land: the pane, and the edge it splits off
/// to, or `None` to join the pane.
#[derive(Clone, Copy, PartialEq)]
pub(in crate::ui) struct TabDrop {
    pane: PaneId,
    edge: Option<Edge>,
}

/// Makes `element`, the content of `pane`, take dropped tabs.
pub(super) fn accept_tabs(element: Div, pane: PaneId, cx: &Context<Workspace>) -> Div {
    element
        .on_drag_move::<DraggedTab>(cx.listener(
            move |this, event: &DragMoveEvent<DraggedTab>, _, cx| {
                let (bounds, position) = (event.bounds, event.event.position);
                let target = bounds.contains(&position).then(|| {
                    let x = (position.x - bounds.left()) / bounds.size.width;
                    let y = (position.y - bounds.top()) / bounds.size.height;
                    tabs::drop_edge(x, y)
                });
                let id = event.drag(cx).id.clone();
                this.hover_drop(pane, &id, target, cx);
            },
        ))
        .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
            let target = this.tab_drop.take().filter(|drop| drop.pane == pane);
            this.tab_drag = false;
            match target {
                Some(TabDrop {
                    edge: Some(edge), ..
                }) => this.split_tab(&dragged.id, pane, edge, window, cx),
                Some(TabDrop { edge: None, .. }) => {
                    this.move_tab(&dragged.id, pane, None, window, cx)
                }
                None => cx.notify(),
            }
        }))
}

impl Workspace {
    /// Updates the highlighted landing place from a drag over `pane`;
    /// `target` is `None` outside it.
    fn hover_drop(
        &mut self,
        pane: PaneId,
        id: &str,
        target: Option<Option<Edge>>,
        cx: &mut Context<Self>,
    ) {
        let next = match target {
            Some(edge) => {
                let layout = self.tab_layout(cx);
                let useful = match edge {
                    Some(_) => layout.can_split(id, pane),
                    None => layout.pane_of(id) != Some(pane),
                };
                useful.then_some(TabDrop { pane, edge })
            }
            None if self.tab_drop.is_some_and(|drop| drop.pane != pane) => return,
            None => None,
        };
        if self.tab_drop != next {
            self.tab_drop = next;
            cx.notify();
        }
    }

    /// The highlighted part of `pane` while a tab is dragged over it.
    pub(super) fn drop_highlight(&self, pane: PaneId, cx: &App) -> Option<Div> {
        let drop = self
            .tab_drop
            .filter(|drop| drop.pane == pane && self.tab_drag)?;
        let theme = cx.theme();
        let label = match drop.edge {
            None => "Переместить сюда",
            Some(_) => "Разделить",
        };
        let half = relative(0.5);
        let highlight = div()
            .absolute()
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .text_color(theme.muted_foreground)
            .bg(theme.drop_target.opacity(0.35))
            .border_1()
            .border_color(theme.drag_border)
            .child(label);
        Some(match drop.edge {
            None => highlight.inset_0(),
            Some(edge) => highlight
                .map(|zone| match edge {
                    Edge::Left | Edge::Right => zone.top_0().bottom_0().w(half),
                    Edge::Top | Edge::Bottom => zone.left_0().right_0().h(half),
                })
                .map(|zone| match edge {
                    Edge::Left => zone.left_0(),
                    Edge::Right => zone.right_0(),
                    Edge::Top => zone.top_0(),
                    Edge::Bottom => zone.bottom_0(),
                }),
        })
    }
}
