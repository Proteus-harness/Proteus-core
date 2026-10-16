//! Virtualized hierarchical rows; domain actions belong to the process.

use crate::declarative::tree::Row;
use crate::ui::{fade::faded, kit::Pressable as _, prefs};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;

pub(super) struct Tree {
    pub rows: Vec<Row>,
    cursor: usize,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    action: Rc<dyn Fn(String, &'static str, &mut App)>,
}

impl Tree {
    pub fn new(
        rows: Vec<Row>,
        cx: &mut Context<Self>,
        action: impl Fn(String, &'static str, &mut App) + 'static,
    ) -> Self {
        Self {
            rows,
            cursor: 0,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            action: Rc::new(action),
        }
    }

    pub fn set_rows(&mut self, rows: Vec<Row>, cx: &mut Context<Self>) {
        if self.rows == rows {
            return;
        }
        let previous = self.rows.get(self.cursor).map(|row| row.id.as_str());
        self.cursor = rows
            .iter()
            .position(|row| row.selected)
            .or_else(|| {
                rows.iter()
                    .position(|row| Some(row.id.as_str()) == previous)
            })
            .unwrap_or(self.cursor)
            .min(rows.len().saturating_sub(1));
        self.rows = rows;
        cx.notify();
    }

    pub fn focus(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn act(&self, event: &'static str, cx: &mut App) {
        if let Some(row) = self.rows.get(self.cursor) {
            (self.action)(row.id.clone(), event, cx);
        }
    }
}

impl Render for Tree {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
        let hover = cx.theme().list_hover;
        let selected = cx.theme().primary.opacity(0.12);
        let list = uniform_list(
            "extension-tree",
            self.rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|index| {
                        let row = &this.rows[index];
                        let color = super::render::tone_color(row.tone, cx);
                        let icon = match row.expanded {
                            Some(true) => IconName::FolderOpen,
                            Some(false) => IconName::Folder,
                            None => IconName::File,
                        };
                        h_flex()
                            .id(("tree-row", index))
                            .h(prefs::scaled(26.))
                            .gap_1p5()
                            .pr_2()
                            .pl(prefs::scaled(8. + row.depth as f32 * 14.))
                            .text_sm()
                            .text_color(color)
                            .cursor_pointer()
                            .pressable(cx)
                            .when(index == this.cursor || row.selected, |row| row.bg(selected))
                            .hover(move |row| row.bg(hover))
                            .child(Icon::new(icon).small())
                            .child(div().flex_1().min_w_0().child(faded(row.label.clone())))
                            .child(div().text_xs().child(row.detail.clone()))
                            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                                this.cursor = index;
                                this.act(
                                    if event.click_count() > 1 {
                                        "open"
                                    } else {
                                        "select"
                                    },
                                    cx,
                                );
                                cx.notify();
                            }))
                            .into_any_element()
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scroll)
        .size_full();
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.focus.focus(window, cx)),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let last = this.rows.len().saturating_sub(1);
                match event.keystroke.key.as_str() {
                    "up" => this.cursor = this.cursor.saturating_sub(1),
                    "down" => this.cursor = (this.cursor + 1).min(last),
                    "home" => this.cursor = 0,
                    "end" => this.cursor = last,
                    "pageup" => this.cursor = this.cursor.saturating_sub(12),
                    "pagedown" => this.cursor = (this.cursor + 12).min(last),
                    "space" => {
                        this.act("select", cx);
                        cx.stop_propagation();
                        return;
                    }
                    "enter" => {
                        this.act("open", cx);
                        cx.stop_propagation();
                        return;
                    }
                    "left" => {
                        this.act("collapse", cx);
                        cx.stop_propagation();
                        return;
                    }
                    "right" => {
                        this.act("expand", cx);
                        cx.stop_propagation();
                        return;
                    }
                    _ => return,
                }
                this.scroll
                    .scroll_to_item(this.cursor, ScrollStrategy::Nearest);
                this.act("preview", cx);
                cx.notify();
                cx.stop_propagation();
            }))
            .child(list)
    }
}
