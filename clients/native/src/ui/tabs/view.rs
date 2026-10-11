use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{drop, split};
use crate::tabs::{self, Axis, Node, Pane, PaneId, Size, TabLayout};
use crate::ui::motion;
use crate::ui::workspace::Workspace;

impl Workspace {
    /// The tree of panes with their bars, contents and dividers.
    pub(in crate::ui) fn render_tabs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let layout = self.tab_layout(cx);
        if !cx.has_active_drag() {
            self.tab_drag = false;
            self.tab_drop = None;
        }
        self.sync_tab_strips(&layout);
        div().flex_1().min_w_0().h_full().child(self.render_node(
            &layout.root,
            Vec::new(),
            &layout,
            window,
            cx,
        ))
    }

    /// Scrolls a newly active tab into view in its strip.
    fn sync_tab_strips(&mut self, layout: &TabLayout) {
        let panes = layout.panes();
        self.tab_strips
            .retain(|id, _| panes.iter().any(|pane| pane.id == *id));
        for pane in panes {
            let (strip, shown) = self.tab_strips.entry(pane.id).or_default();
            if *shown != pane.active {
                if let Some(ix) = pane
                    .active
                    .as_ref()
                    .and_then(|active| pane.tabs.iter().position(|id| id == active))
                {
                    strip.scroll_to_item(ix);
                }
                shown.clone_from(&pane.active);
            }
        }
    }

    fn render_node(
        &mut self,
        node: &Node,
        path: Vec<usize>,
        layout: &TabLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let split = match node {
            Node::Pane(pane) => {
                return self
                    .render_pane(pane, layout, window, cx)
                    .into_any_element();
            }
            Node::Split(split) => split,
        };
        let axis = split.axis;
        let container = match axis {
            Axis::Row => h_flex(),
            Axis::Column => v_flex(),
        };
        let mut container = split::track_resize(
            container.size_full().min_w_0().min_h_0(),
            path.clone(),
            axis,
            cx,
        );
        for (ix, child) in split.children.iter().enumerate() {
            if ix > 0 {
                container = container.child(split::divider(path.clone(), ix - 1, axis, cx));
            }
            let mut child_path = path.clone();
            child_path.push(ix);
            let content = self.render_node(&child.node, child_path, layout, window, cx);
            container = container.child(
                div()
                    .map(|cell| match child.size {
                        Size::Weight(weight) => cell.flex_basis(relative(0.)).flex_grow(weight),
                        Size::Fixed(px_len) => cell.flex_basis(px(px_len)).flex_grow(0.),
                    })
                    .flex_shrink(1.)
                    .min_w_0()
                    .min_h_0()
                    .map(|cell| match axis {
                        Axis::Row => cell.h_full(),
                        Axis::Column => cell.w_full(),
                    })
                    .child(content),
            );
        }
        container.into_any_element()
    }

    fn render_pane(
        &mut self,
        pane: &Pane,
        layout: &TabLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = pane.id;
        let content = match pane.active.as_deref() {
            Some(tab) if tabs::is_panel(tab) => self.render_panel(tab, window, cx),
            Some(tab) if tabs::is_chat(tab) => match self.chats.get(tab) {
                Some(chat) => div()
                    .size_full()
                    .child(chat.view.clone())
                    .into_any_element(),
                None => div().into_any_element(),
            },
            Some(tab) => match self.ensure_document(tab, window, cx) {
                Some(document) => motion::rise_in(
                    div().size_full().child(document.view),
                    ElementId::Name(format!("tab-document-{tab}").into()),
                )
                .into_any_element(),
                None => div().into_any_element(),
            },
            None => self.render_empty_pane(id, cx).into_any_element(),
        };
        let theme = cx.theme();
        let ring = (layout.pane_count() > 1 && layout.focused == id).then(|| {
            div()
                .absolute()
                .inset_0()
                .border_1()
                .border_color(theme.primary.opacity(0.35))
        });
        let body = div().relative().flex_1().min_h_0().child(content);
        v_flex()
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| this.focus_pane(id, cx)),
            )
            .child(self.render_tab_bar(pane, layout, cx))
            .child(drop::accept_tabs(body, id, cx).children(self.drop_highlight(id, cx)))
            .children(ring)
    }

    /// The only pane, without tabs, explains how to fill it.
    fn render_empty_pane(&self, pane: PaneId, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .px_6()
            .child(
                div()
                    .text_sm()
                    .text_center()
                    .text_color(theme.muted_foreground)
                    .child("Откройте сессию или файл на панелях «Сессии» и «Файлы»."),
            )
            .children(self.notice.clone().map(|notice| {
                div()
                    .text_xs()
                    .text_center()
                    .text_color(theme.danger)
                    .child(notice)
            }))
            .when(self.chats.is_empty(), |column| {
                column.child(
                    Button::new(("empty-pane-new-session", pane.0 as usize))
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .label("Новая сессия")
                        .on_click(cx.listener(|this, _, window, cx| this.new_session(window, cx))),
                )
            })
    }
}
