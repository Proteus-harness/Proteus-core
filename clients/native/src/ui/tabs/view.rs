use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::message_scroller::MessageScroller;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::bar::DraggedTab;
use crate::tabs::{CHAT, TabLayout};
use crate::ui::motion;
use crate::ui::thread::CONTENT_WIDTH;
use crate::ui::workspace::Workspace;

/// Narrowest a tab group may get while resizing.
const MIN_GROUP: f32 = 320.;

/// Payload of the divider between tab groups while it is dragged.
pub(in crate::ui) struct SplitDrag;

impl Render for SplitDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Workspace {
    /// Both tab groups with their bars, contents and the divider.
    pub(in crate::ui) fn render_tabs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let layout = self.tab_layout(cx);
        if !cx.has_active_drag() {
            self.tab_drag = false;
        }
        let ratio = f32::from(layout.split) / 1000.;
        let mut row = h_flex()
            .id("tab-groups")
            .flex_1()
            .min_w_0()
            .h_full()
            .on_drag_move::<SplitDrag>(cx.listener(
                |this, event: &DragMoveEvent<SplitDrag>, _, cx| {
                    let bounds = event.bounds;
                    let width = bounds.size.width.as_f32();
                    if width <= 0. {
                        return;
                    }
                    let x = (event.event.position.x - bounds.left()).as_f32();
                    let min = (MIN_GROUP / width).min(0.5);
                    let ratio = (x / width).clamp(min, 1. - min);
                    this.set_split(Some((ratio * 1000.).round() as u16), cx);
                },
            ));
        for group in 0..layout.groups.len() {
            let column = self.render_group(group, &layout, window, cx);
            row = if layout.is_split() && group == 0 {
                row.child(column.flex_none().w(relative(ratio)))
            } else {
                if group == 1 {
                    row = row.child(splitter(cx));
                }
                row.child(column.flex_1())
            };
        }
        row
    }

    fn render_group(
        &mut self,
        group: usize,
        layout: &TabLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let content = match layout.active(group) {
            Some(CHAT) => self.render_chat(window, cx).into_any_element(),
            Some(id) => match self.ensure_document(id, window, cx) {
                Some(document) => motion::rise_in(
                    div().size_full().child(document.view),
                    ElementId::Name(format!("tab-document-{id}").into()),
                )
                .into_any_element(),
                None => div().into_any_element(),
            },
            None => self
                .render_empty_group(group, layout, cx)
                .into_any_element(),
        };
        v_flex()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| this.focus_group(group, cx)),
            )
            .child(self.render_tab_bar(group, layout, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(content)
                    .children(self.drop_zone(group, layout, cx)),
            )
    }

    /// While a tab is dragged: drop it here to move it into this group, or
    /// onto the right edge of a single group to split the area.
    fn drop_zone(
        &self,
        group: usize,
        layout: &TabLayout,
        cx: &Context<Self>,
    ) -> Option<Stateful<Div>> {
        if !self.tab_drag {
            return None;
        }
        let theme = cx.theme();
        let (fill, hint) = (theme.drop_target, theme.muted_foreground);
        let (target, label) = if layout.is_split() {
            (group, "Переместить сюда")
        } else {
            (1, "Разделить область")
        };
        let zone = div()
            .id(("tab-drop-zone", group))
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .text_color(hint)
            .bg(fill.opacity(0.25))
            .drag_over::<DraggedTab>(move |style, _, _, _| style.bg(fill))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                this.tab_drag = false;
                this.move_tab(&dragged.id, target, None, window, cx);
            }))
            .child(label);
        Some(if layout.is_split() {
            zone.left_0()
        } else {
            zone.w(relative(0.35))
        })
    }

    /// A group without tabs explains how to fill it.
    fn render_empty_group(&self, group: usize, layout: &TabLayout, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let chat_elsewhere = layout.group_of(CHAT) != Some(group);
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
                    .child("Откройте файл в боковой панели или перетащите сюда вкладку."),
            )
            .when(chat_elsewhere, |column| {
                column.child(
                    Button::new(("empty-group-chat", group))
                        .ghost()
                        .small()
                        .icon(Icon::new(gpui_kit::assets::IconName::MessageSquare))
                        .label("Перенести чат сюда")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_tab_in(CHAT, group, window, cx)
                        })),
                )
            })
            .child(
                Button::new(("empty-group-close", group))
                    .ghost()
                    .xsmall()
                    .label("Закрыть область")
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_split(window, cx))),
            )
    }

    /// The conversation: thread, pending requests, notice and composer.
    fn render_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let renderer = cx.processor(|this, ix: usize, window, cx| this.render_row(ix, window, cx));
        // A new id per session replays the fade when switching sessions.
        let thread_id = ElementId::NamedInteger("thread-enter".into(), self.session_key());
        v_flex()
            .size_full()
            .child(motion::fade_in_if(
                MessageScroller::new("thread", self.scroller.clone(), renderer)
                    .flex_1()
                    .min_h_0()
                    .scrollbar(true)
                    .jump_button(true)
                    .with_row_style(StyleRefinement::default().px_0().pb_0()),
                thread_id,
                true,
            ))
            .children(self.render_requests(window, cx))
            .children(self.notice.clone().map(|notice| {
                h_flex().w_full().justify_center().px_6().pt_2().child(
                    div()
                        .w_full()
                        .max_w(px(CONTENT_WIDTH))
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(notice),
                )
            }))
            .child(self.render_composer(cx))
    }
}

fn splitter(cx: &Context<Workspace>) -> impl IntoElement {
    let theme = cx.theme();
    let (line, hover) = (theme.border, theme.drag_border);
    h_flex()
        .id("tab-splitter")
        .w(px(5.))
        .h_full()
        .flex_none()
        .justify_center()
        .cursor(CursorStyle::ResizeLeftRight)
        .child(div().w(px(1.)).h_full().bg(line))
        .hover(move |style| style.bg(hover.opacity(0.4)))
        .on_drag(SplitDrag, |_, _, _, cx| cx.new(|_| SplitDrag))
        .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
            if event.click_count() == 2 {
                this.reset_split(cx);
            }
        }))
}
