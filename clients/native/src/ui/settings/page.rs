//! Settings content: page column with grouped cards and the floating save bar.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::SettingsView;
use super::nav::changed_dot;
use super::rows::hashed_id;
use crate::settings::{Group, Item, Page, search};
use crate::ui::motion::{chevron, rise_in, slide_up};

const COLUMN_WIDTH: f32 = 720.;

impl SettingsView {
    pub(super) fn render_content(&self, pages: &[Page], cx: &Context<Self>) -> impl IntoElement {
        let (key, column) = if self.query.is_empty() {
            let page = self.current_page(pages);
            let key = format!("page-{}", page.map(|page| page.id.as_str()).unwrap_or(""));
            let first = page.is_some_and(|page| pages.first().is_some_and(|f| f.id == page.id));
            (key, self.render_page(page, first, cx))
        } else {
            ("search".to_string(), self.render_search(pages, cx))
        };
        let column = rise_in(
            column.w_full().max_w(px(COLUMN_WIDTH)),
            ElementId::Name(format!("settings-{key}").into()),
        );

        div()
            .relative()
            .size_full()
            .child(
                v_flex()
                    .size_full()
                    .items_center()
                    .px_8()
                    .pt_8()
                    .pb(px(112.))
                    .child(column)
                    .overflow_y_scrollbar()
                    .id(ElementId::Name(format!("settings-scroll-{key}").into())),
            )
            .children(self.render_save_bar(cx))
    }

    fn render_page(&self, page: Option<&Page>, first: bool, cx: &Context<Self>) -> Div {
        let Some(page) = page else {
            return v_flex();
        };
        let theme = cx.theme();
        v_flex()
            .gap_6()
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_xl().font_semibold().child(page.title.clone()))
                    .children(page.description.clone().map(|text| {
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(text)
                    })),
            )
            .when(first, |column| column.children(self.render_warnings(cx)))
            .children(page.groups.iter().map(|group| self.render_group(group, cx)))
    }

    fn render_search(&self, pages: &[Page], cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let groups = search(pages, &self.query);
        let found: usize = groups.iter().map(|group| group.items.len()).sum();
        v_flex()
            .gap_6()
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_xl().font_semibold().child("Поиск"))
                    .child(div().text_sm().text_color(theme.muted_foreground).child(
                        if found == 0 {
                            format!("Ничего не найдено по запросу «{}»", self.query)
                        } else {
                            format!("Найдено: {found}")
                        },
                    )),
            )
            .children(groups.iter().map(|group| self.render_group(group, cx)))
    }

    fn render_warnings(&self, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let snapshot = self.snapshot()?;
        if snapshot.warnings.is_empty() {
            return None;
        }
        let theme = cx.theme();
        Some(
            v_flex()
                .gap_1p5()
                .px_4()
                .py_3()
                .rounded_lg()
                .border_1()
                .border_color(theme.warning.opacity(0.4))
                .bg(theme.warning.opacity(0.08))
                .text_sm()
                .children(snapshot.warnings.iter().map(|warning| {
                    h_flex()
                        .gap_2()
                        .items_start()
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .small()
                                .mt_0p5()
                                .text_color(theme.warning),
                        )
                        .child(div().flex_1().min_w_0().child(warning.message.clone()))
                })),
        )
    }

    fn render_group(&self, group: &Group, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let changed = group.changed(&self.draft, &self.baseline);
        let heading = (group.title.is_some() || group.description.is_some()).then(|| {
            v_flex()
                .gap_0p5()
                .px_1()
                .children(group.title.clone().map(|title| {
                    h_flex()
                        .gap_2()
                        .items_center()
                        .text_sm()
                        .font_semibold()
                        .child(title)
                        .when(changed, |title| title.child(changed_dot(cx)))
                }))
                .children(group.description.clone().map(|text| {
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(text)
                }))
        });

        let mut rows: Vec<AnyElement> = group
            .items
            .iter()
            .map(|item| self.render_item(item, cx))
            .collect();
        if !group.advanced.is_empty() {
            let open = self.open_advanced.contains(&group.key);
            rows.push(self.render_advanced_toggle(group, open, cx));
            if open {
                rows.push(
                    rise_in(
                        divided(self.render_items(&group.advanced, cx), cx),
                        hashed_id("settings-advanced-rows", &group.key),
                    )
                    .into_any_element(),
                );
            }
        }

        v_flex().gap_2().children(heading).child(
            divided(rows, cx)
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .bg(theme.group_box)
                .overflow_hidden(),
        )
    }

    fn render_items(&self, items: &[Item], cx: &Context<Self>) -> Vec<AnyElement> {
        items
            .iter()
            .map(|item| self.render_item(item, cx))
            .collect()
    }

    fn render_advanced_toggle(&self, group: &Group, open: bool, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let changed = group
            .advanced
            .iter()
            .any(|item| item.changed(&self.draft, &self.baseline));
        let key = group.key.clone();
        h_flex()
            .id(hashed_id("settings-advanced", &group.key))
            .px_4()
            .py_2p5()
            .gap_2()
            .items_center()
            .text_sm()
            .text_color(theme.muted_foreground)
            .cursor_pointer()
            .hover(|row| row.bg(theme.list_hover))
            .child(chevron(
                hashed_id("settings-advanced-chevron", &group.key),
                open,
                theme.muted_foreground,
            ))
            .child(format!("Дополнительно · {}", group.advanced.len()))
            .when(changed && !open, |row| row.child(changed_dot(cx)))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_advanced(&key, cx)))
            .into_any_element()
    }

    fn render_save_bar(&self, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let theme = cx.theme();
        let dirty = self.dirty();
        if !dirty && !self.saving && self.message.is_none() {
            return None;
        }
        let bar = h_flex()
            .w_full()
            .max_w(px(COLUMN_WIDTH))
            .px_4()
            .py_2p5()
            .gap_3()
            .items_center()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_lg()
            .text_sm();
        let bar = if dirty || self.saving {
            let hint = if let Some(message) = self.message.as_ref().filter(|m| m.error) {
                Some((message.text.clone(), theme.danger))
            } else if self.has_errors() {
                Some(("Исправьте ошибки в полях".into(), theme.danger))
            } else if self.snapshot().is_some_and(|snapshot| !snapshot.writable) {
                Some(("Профиль только для чтения".into(), theme.warning))
            } else {
                None
            };
            bar.child(changed_dot(cx))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child("Несохранённые изменения")
                        .children(
                            hint.map(|(text, color)| div().text_xs().text_color(color).child(text)),
                        ),
                )
                .child(
                    Button::new("settings-discard")
                        .label("Отменить")
                        .ghost()
                        .small()
                        .disabled(!dirty || self.saving)
                        .on_click(cx.listener(|this, _, _, cx| this.discard(cx))),
                )
                .child(
                    Button::new("settings-save")
                        .label("Сохранить")
                        .primary()
                        .small()
                        .loading(self.saving)
                        .disabled(!self.can_save())
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                )
        } else {
            let message = self.message.as_ref()?;
            let (icon, color) = if message.error {
                (IconName::CircleX, theme.danger)
            } else {
                (IconName::CircleCheck, theme.success)
            };
            bar.child(Icon::new(icon).small().text_color(color))
                .child(div().flex_1().min_w_0().child(message.text.clone()))
        };

        Some(
            h_flex()
                .absolute()
                .bottom_4()
                .left_0()
                .right_0()
                .px_8()
                .justify_center()
                .child(slide_up(bar, "settings-save-bar")),
        )
    }
}

/// Stacks rows with hairline dividers between them.
fn divided(rows: Vec<AnyElement>, cx: &App) -> Div {
    let border = cx.theme().border;
    v_flex().children(rows.into_iter().enumerate().map(move |(ix, row)| {
        div()
            .when(ix > 0, |cell| cell.border_t_1().border_color(border))
            .child(row)
    }))
}
