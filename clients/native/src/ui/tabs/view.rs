use crate::ui::kit;
use std::time::Instant;

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::motion::Frame;
use super::{drop, split};
use crate::tabs::{self, Axis, Node, Pane, PaneId, Size, TabLayout};
use crate::ui::workspace::Workspace;
use crate::ui::{motion, prefs};

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
        let now = Instant::now();
        let running = self.tab_motion.tick(now);
        let frames = self.tab_area.get().and_then(|area| {
            // The layout is measured at 100% scale.
            let scale = prefs::scale(cx);
            let size = (area.width.as_f32() / scale, area.height.as_f32() / scale);
            let target = layout.rects_spaced(size.0, size.1, split::DIVIDER);
            let panes = self.tab_motion.panes();
            panes.retarget(size, target, self.split_drag.is_some(), now);
            panes.sample(now)
        });
        if running || frames.is_some() {
            window.request_animation_frame();
        }
        let area = self.tab_area.clone();
        let probe = canvas(
            move |bounds, _, _| area.set(Some(bounds.size)),
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let content = match frames {
            Some(frames) => self.render_moving_panes(&frames, &layout, window, cx),
            None => self.render_node(&layout.root, Vec::new(), &layout, window, cx),
        };
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(probe)
            .child(content)
    }

    /// Panes placed where they are while they glide to a new arrangement.
    /// Each keeps its final size and is clipped by its moving frame, so
    /// contents do not reflow on every frame; dividers are thin lines.
    fn render_moving_panes(
        &mut self,
        frames: &[Frame],
        layout: &TabLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let line = cx.theme().border;
        let half = split::DIVIDER / 2.;
        let mut area = div().relative().size_full();
        for frame in frames {
            let Some(pane) = layout.pane(frame.id) else {
                continue;
            };
            let (rect, target) = (frame.rect, frame.target);
            let content = self.render_pane(pane, layout, window, cx);
            area = area.child(
                div()
                    .absolute()
                    .left(prefs::scaled(rect.x))
                    .top(prefs::scaled(rect.y))
                    .w(prefs::scaled(rect.w))
                    .h(prefs::scaled(rect.h))
                    .overflow_hidden()
                    .opacity(frame.opacity)
                    .child(
                        div()
                            .w(prefs::scaled(target.w))
                            .h(prefs::scaled(target.h))
                            .child(content),
                    ),
            );
            if rect.x > half {
                area = area.child(
                    div()
                        .absolute()
                        .left(prefs::scaled(rect.x - half))
                        .top(prefs::scaled(rect.y))
                        .w(px(1.))
                        .h(prefs::scaled(rect.h))
                        .bg(line),
                );
            }
            if rect.y > half {
                area = area.child(
                    div()
                        .absolute()
                        .left(prefs::scaled(rect.x))
                        .top(prefs::scaled(rect.y - half))
                        .w(prefs::scaled(rect.w))
                        .h(px(1.))
                        .bg(line),
                );
            }
        }
        area.into_any_element()
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
                        Size::Fixed(len) => cell.flex_basis(prefs::scaled(len)).flex_grow(0.),
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
                Some(document) => motion::rise_in_if(
                    div().size_full().child(document.view),
                    ElementId::Name(format!("tab-document-{tab}").into()),
                    self.tab_motion.opened(tab, Instant::now()),
                )
                .into_any_element(),
                None => div().into_any_element(),
            },
            None => self.render_empty_pane(id, cx).into_any_element(),
        };
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
                    kit::button(("empty-pane-new-session", pane.0 as usize))
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .label("Новая сессия")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.new_session(Some(pane), window, cx)
                        })),
                )
            })
    }
}
