//! Page frame: title, description, scrolled body and the save bar.

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::widgets::changed_dot;
use super::{LoadState, SettingsView};
use crate::settings::catalog::{PageId, area_labels, page_description, page_title};
use crate::ui::extensions;
use crate::ui::fade::faded;
use crate::ui::floor::floor;
use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::motion::Spinner;
use crate::ui::motion::switch_in;
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
                                kit::button("settings-retry")
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
        if let PageId::Extension(id) = &self.page
            && extensions::is_enabled(id, cx)
            && let Some(page) = extensions::find(id).and_then(|manifest| manifest.page)
        {
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
            PageId::Tools => self.tools_page(cx),
            PageId::Hooks => self.hooks_page(cx),
            PageId::Modules => self.modules_page(cx),
            PageId::Mcp => self.mcp_page(cx),
            PageId::Skills => self.skills_page(cx),
            PageId::Packages => self.packages_page(cx),
            PageId::History => self.history_page(cx),
            PageId::Appearance => self.appearance_page(cx),
            PageId::Chat => self.chat_page(cx),
            PageId::Shortcuts => self.shortcuts_page(cx),
            PageId::Extensions => self.extensions_page(cx),
            PageId::Extension(id) => self.extension_page(id, cx),
        };
        let (title, description, switch) = match &self.page {
            PageId::Extension(id) => {
                let manifest = extensions::find(id);
                (
                    manifest.map_or_else(|| id.clone(), |manifest| manifest.title.to_string()),
                    manifest
                        .map_or("", |manifest| manifest.description)
                        .to_string(),
                    self.extension_switch(id, cx),
                )
            }
            page => (
                page_title(snapshot, page),
                page_description(snapshot, page),
                None,
            ),
        };
        let key = format!("{:?}", self.page);
        let column = v_flex()
            .w_full()
            .max_w(prefs::scaled(COLUMN_WIDTH))
            .pb_10()
            .gap_6()
            .child(
                h_flex()
                    .gap_4()
                    .items_start()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(div().text_xl().font_semibold().child(title))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(description),
                            ),
                    )
                    .children(switch.map(|switch| div().flex_none().pt_1().child(switch))),
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
            // The bottom padding is part of the page, so the floor that
            // keeps the scroll position counts it.
            .child(floor(
                ElementId::Name(format!("settings-floor-{key}").into()),
                switch_in(
                    column,
                    ElementId::Name(format!("settings-page-{key}").into()),
                ),
            ))
            .overflow_y_scrollbar()
            .id(ElementId::Name(format!("settings-scroll-{key}").into()));
        v_flex()
            .size_full()
            .child(div().flex_1().min_h_0().child(scroll))
            .when(self.page.is_agent(), |frame| {
                frame.child(self.render_save_bar(cx))
            })
            .when(self.page == PageId::Extensions, |frame| {
                frame.on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                    this.install_archives(paths.paths(), cx)
                }))
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
                .rounded(theme.radius_lg)
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
                            .child(div().child(faded(status))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(faded(if read_only {
                                format!("{target} · только чтение")
                            } else {
                                target
                            })),
                    ),
            )
            .when(!dirty, |bar| {
                bar.child(
                    kit::button("settings-reload")
                        .label("Обновить")
                        .ghost()
                        .small()
                        .disabled(self.saving)
                        .hint("Перечитать профиль", cx)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.load(cx);
                            cx.notify();
                        })),
                )
            })
            .child(
                kit::button("settings-discard")
                    .label("Сбросить")
                    .ghost()
                    .small()
                    .disabled(!dirty || self.saving)
                    .on_click(cx.listener(|this, _, _, cx| this.discard(cx))),
            )
            .child(
                kit::button("settings-save")
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
