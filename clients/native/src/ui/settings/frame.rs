//! Page frame: title, description, scrolled body and the save bar.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::widgets::changed_dot;
use super::{LoadState, SettingsView};
use crate::settings::catalog::{PageId, area_labels, page_description, page_title};
use crate::ui::motion::rise_in;
use crate::ui::prefs;

const COLUMN_WIDTH: f32 = 720.;

impl SettingsView {
    pub(super) fn render_frame(&self, cx: &Context<Self>) -> AnyElement {
        if self.page.is_agent() {
            match &self.state {
                LoadState::Loading => {
                    return centered(
                        h_flex()
                            .gap_2()
                            .text_color(cx.theme().muted_foreground)
                            .child(Spinner::new())
                            .child("Загружаю профиль агента…"),
                    );
                }
                LoadState::Failed(error) => {
                    return centered(
                        v_flex()
                            .gap_3()
                            .items_center()
                            .max_w(prefs::scaled(560.))
                            .child(div().text_color(cx.theme().danger).child(error.clone()))
                            .child(
                                Button::new("settings-retry")
                                    .label("Повторить")
                                    .outline()
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.load(cx);
                                        cx.notify();
                                    })),
                            ),
                    );
                }
                LoadState::Ready(_) => {}
            }
        }
        if let PageId::Overview(page) = self.page {
            return div()
                .size_full()
                .px_8()
                .pt_6()
                .pb_4()
                .child(self.overview.view(page))
                .into_any_element();
        }
        let theme = cx.theme();
        let snapshot = self.snapshot();
        let body = match &self.page {
            PageId::Model => self.model_page(cx),
            PageId::Slot(slot) => self.slot_page(slot, cx),
            PageId::ContextProviders => self.context_providers_page(cx),
            PageId::Tools => self.tools_page(cx),
            PageId::Hooks => self.hooks_page(cx),
            PageId::Plugins => self.plugins_page(cx),
            PageId::History => self.history_page(cx),
            PageId::Appearance => self.appearance_page(cx),
            PageId::Chat => self.chat_page(cx),
            PageId::Extensions => self.extensions_page(cx),
            PageId::Overview(_) => unreachable!("overview pages render their own frame"),
        };
        let key = format!("{:?}", self.page);
        let column = v_flex()
            .w_full()
            .max_w(prefs::scaled(COLUMN_WIDTH))
            .gap_6()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xl()
                            .font_semibold()
                            .child(page_title(snapshot, &self.page)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(page_description(snapshot, &self.page)),
                    ),
            )
            .children(
                (self.page == PageId::Model)
                    .then(|| self.render_warnings(cx))
                    .flatten(),
            )
            .children(body);
        let scroll = v_flex()
            .size_full()
            .items_center()
            .px_8()
            .pt_8()
            .pb_10()
            .child(rise_in(
                column,
                ElementId::Name(format!("settings-page-{key}").into()),
            ))
            .overflow_y_scrollbar()
            .id(ElementId::Name(format!("settings-scroll-{key}").into()));
        v_flex()
            .size_full()
            .child(div().flex_1().min_h_0().child(scroll))
            .when(self.page.is_agent(), |frame| {
                frame.child(self.render_save_bar(cx))
            })
            .into_any_element()
    }

    fn render_warnings(&self, cx: &Context<Self>) -> Option<AnyElement> {
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
                .children(snapshot.warnings.iter().map(|warning| {
                    h_flex()
                        .gap_2()
                        .items_start()
                        .text_sm()
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .small()
                                .flex_none()
                                .mt_0p5()
                                .text_color(theme.warning),
                        )
                        .child(div().flex_1().min_w_0().child(warning.message.clone()))
                }))
                .into_any_element(),
        )
    }

    fn render_save_bar(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let snapshot = self.snapshot();
        let dirty = self.dirty();
        let (status, tone) = if self.saving {
            (
                "Сохраняю и проверяю сборку…".to_string(),
                theme.muted_foreground,
            )
        } else if self.editors.has_errors() {
            ("Исправьте отмеченные значения".to_string(), theme.danger)
        } else if let Some(feedback) = self.feedback.as_ref().filter(|feedback| feedback.error) {
            (feedback.text.to_string(), theme.danger)
        } else if dirty {
            let areas = snapshot
                .map(|snapshot| area_labels(snapshot, &self.draft.areas(&self.baseline)))
                .unwrap_or_default();
            (
                format!("Не сохранено: {}", areas.join(", ")),
                theme.foreground,
            )
        } else if let Some(feedback) = &self.feedback {
            (feedback.text.to_string(), theme.success)
        } else {
            ("Изменений нет".to_string(), theme.muted_foreground)
        };
        let target = match snapshot.and_then(|snapshot| snapshot.target_path.clone()) {
            Some(path) => home_relative(&path),
            None => "Файл профиля недоступен".to_string(),
        };
        let read_only = snapshot.is_some_and(|snapshot| !snapshot.writable);
        h_flex()
            .flex_none()
            .h(prefs::scaled(56.))
            .px_6()
            .gap_3()
            .items_center()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_sm()
                            .text_color(tone)
                            .when(dirty && !self.saving, |line| line.child(changed_dot(cx)))
                            .child(div().truncate().child(status)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(if read_only {
                                format!("{target} · только чтение")
                            } else {
                                target
                            }),
                    ),
            )
            .when(!dirty, |bar| {
                bar.child(
                    Button::new("settings-reload")
                        .label("Обновить")
                        .ghost()
                        .small()
                        .disabled(self.saving)
                        .tooltip("Перечитать профиль с диска")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.load(cx);
                            cx.notify();
                        })),
                )
            })
            .child(
                Button::new("settings-discard")
                    .label("Сбросить")
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
    }
}

fn centered(content: impl IntoElement) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p_8()
        .child(content)
        .into_any_element()
}

fn home_relative(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_string(),
    }
}
