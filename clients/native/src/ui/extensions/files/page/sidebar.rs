use std::ops::Range;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{FilesPage, Tab};
use crate::files::{ChangeStatus, EntryKind};
use crate::ui::extensions::files::{path_label, status_color};

const TREE_WIDTH: f32 = 320.;
const TREE_MIN_WIDTH: f32 = 200.;
const ROW_HEIGHT: f32 = 26.;
const INDENT: f32 = 14.;

impl FilesPage {
    fn row_background(&self, path: &str, focused: bool, cx: &App) -> Option<Hsla> {
        let theme = cx.theme();
        if self.selected.as_deref() == Some(path) {
            Some(theme.list_active)
        } else if focused && self.cursor.as_deref() == Some(path) {
            Some(theme.list_hover)
        } else {
            None
        }
    }

    fn render_row(&self, index: usize, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let row = &self.rows[index];
        let theme = cx.theme();
        let directory = row.kind == EntryKind::Directory;
        let background = self.row_background(&row.path, focused, cx);
        let name_color = match row.status {
            Some(status) => status_color(status, cx),
            None if row.contains_changes => theme.warning,
            None => theme.foreground,
        };
        let leading = if row.loading {
            Spinner::new().xsmall().into_any_element()
        } else if directory {
            Icon::new(if row.expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .xsmall()
            .text_color(theme.muted_foreground)
            .into_any_element()
        } else {
            div().size_3().into_any_element()
        };
        let icon = match (directory, row.expanded) {
            (true, true) => IconName::FolderOpen,
            (true, false) => IconName::Folder,
            (false, _) => IconName::File,
        };
        h_flex()
            .id(("file-row", index))
            .w_full()
            .h(px(ROW_HEIGHT))
            .pl(px(8. + row.depth as f32 * INDENT))
            .pr_3()
            .gap_1p5()
            .text_sm()
            .cursor_pointer()
            .map(|line| match background {
                Some(color) => line.bg(color),
                None => line.hover(|style| style.bg(theme.list_hover)),
            })
            .child(leading)
            .child(
                Icon::new(icon)
                    .small()
                    .flex_none()
                    .text_color(theme.muted_foreground),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(name_color)
                    .when(row.status == Some(ChangeStatus::Deleted), |name| {
                        name.line_through()
                    })
                    .child(row.name.clone()),
            )
            .when(row.truncated, |line| {
                line.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("список обрезан"),
                )
            })
            .when_some(row.status, |line, status| {
                line.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(status_color(status, cx))
                        .child(status.mark()),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| this.click_row(index, window, cx)))
            .into_any_element()
    }

    pub(super) fn render_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let (border, danger) = (cx.theme().border, cx.theme().danger);
        let change_count = self.changes.as_ref().map(|changes| changes.entries.len());
        let tabs = h_flex()
            .gap_1()
            .child(
                tab_button("files-tab-tree", "Дерево", self.tab == Tab::Tree)
                    .on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Tree, cx))),
            )
            .child(
                tab_button(
                    "files-tab-changes",
                    match change_count {
                        Some(count) if count > 0 => format!("Изменения · {count}"),
                        _ => "Изменения".to_string(),
                    },
                    self.tab == Tab::Changes,
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Changes, cx))),
            )
            .child(div().flex_1())
            .child(
                Button::new("files-refresh")
                    .icon(IconName::RefreshCw)
                    .ghost()
                    .xsmall()
                    .tooltip("Обновить")
                    .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
            );
        let focused = self.focus_handle.contains_focused(window, cx);
        let list = match self.tab {
            Tab::Tree if self.rows.is_empty() && !self.query.is_empty() => {
                note("Совпадений в открытых папках нет.", cx).into_any_element()
            }
            Tab::Tree => uniform_list(
                "files-tree",
                self.rows.len(),
                cx.processor(move |this, range: Range<usize>, _, cx| {
                    range
                        .map(|index| this.render_row(index, focused, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.list)
            .flex_1()
            .min_h_0()
            .pb_2()
            .into_any_element(),
            Tab::Changes => self.render_changes(focused, cx).into_any_element(),
        };
        let list = self
            .list_actions(v_flex().flex_1().min_h_0(), cx)
            .track_focus(&self.focus_handle)
            .child(list);
        v_flex()
            .w(relative(0.4))
            .min_w(px(TREE_MIN_WIDTH))
            .max_w(px(TREE_WIDTH))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(border)
            .child(div().px_3().py_2().child(tabs))
            .when_some(self.error.clone(), |column, error| {
                column.child(
                    div()
                        .px_3()
                        .pb_2()
                        .text_xs()
                        .text_color(danger)
                        .child(error),
                )
            })
            .when(self.tab == Tab::Tree, |column| {
                column.child(
                    div().px_3().pb_2().child(
                        Input::new(&self.filter)
                            .small()
                            .cleanable(true)
                            .prefix(Icon::new(IconName::Search).small()),
                    ),
                )
            })
            .child(list)
    }

    fn render_changes(&self, focused: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let list = v_flex()
            .id("files-changes")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.changes_scroll)
            .pb_2();
        let Some(changes) = &self.changes else {
            return list.child(note("Загрузка…", cx));
        };
        if !changes.repository {
            return list.child(note("Это не git-репозиторий.", cx));
        }
        if changes.entries.is_empty() {
            return list.child(note("Изменений нет.", cx));
        }
        let rows = changes.entries.iter().enumerate().map(|(index, change)| {
            let path = change.path.clone();
            let background = self.row_background(&change.path, focused, cx);
            h_flex()
                .id(("files-change", index))
                .h(px(ROW_HEIGHT))
                .flex_none()
                .px_3()
                .gap_2()
                .text_sm()
                .cursor_pointer()
                .map(|row| match background {
                    Some(color) => row.bg(color),
                    None => row.hover(|style| style.bg(theme.list_hover)),
                })
                .child(
                    div()
                        .w_3()
                        .flex_none()
                        .text_xs()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(status_color(change.status, cx))
                        .child(change.status.mark()),
                )
                .child(path_label(&change.path, cx).flex_1())
                .tooltip({
                    let label = change.status.label();
                    move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(label).build(window, cx)
                    }
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.click_change(path.clone(), window, cx)
                }))
        });
        list.children(rows).when(changes.truncated, |list| {
            list.child(note("Показана только часть изменений.", cx))
        })
    }
}

pub(super) fn tab_button(
    id: &'static str,
    label: impl Into<SharedString>,
    selected: bool,
) -> Button {
    Button::new(id)
        .label(label)
        .ghost()
        .xsmall()
        .selected(selected)
}

fn note(text: &'static str, cx: &App) -> Div {
    div()
        .px_3()
        .py_2()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}
