//! Catalog page: header, toolbar, the entry list and the details beside it.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::super::{SettingsView, widgets::hashed_id};
use super::Show;
use super::items::{Item, KINDS, Status, kind_filter, kind_icon};
use crate::catalog::open::OPEN;
use crate::settings::catalog::{PageId, page_description, page_title};
use crate::ui::fade::faded;
use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::prefs;

impl SettingsView {
    pub(in crate::ui::settings) fn catalog_page(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let sources = &prefs::current(cx).catalogs.sources;
        let installed = &prefs::current(cx).catalogs.installed;
        let items = self.catalog_items(installed);
        let updates = items
            .iter()
            .filter(|item| item.status == Status::Update)
            .count();
        let shown = items
            .iter()
            .filter(|item| self.catalog_item_shown(item))
            .collect::<Vec<_>>();
        let selected = self.catalogs.selected.as_ref().and_then(|(source, id)| {
            items
                .iter()
                .find(|item| item.source == source && item.entry.id == *id)
        });
        let body = if sources.is_empty() {
            self.catalog_empty(
                "Источников пока нет",
                "Подключите открытые каталоги skills или свой git-репозиторий.",
                true,
                cx,
            )
        } else if shown.is_empty() {
            self.catalog_empty(
                if items.is_empty() {
                    "Каталоги пусты"
                } else {
                    "Ничего не найдено"
                },
                if items.is_empty() {
                    "Обновите источники или проверьте их в окне «Источники»."
                } else {
                    "Измените поиск или фильтры."
                },
                false,
                cx,
            )
        } else {
            div()
                .id("catalog-list")
                .flex_1()
                .min_w_0()
                .h_full()
                .child(
                    v_flex().children(
                        shown
                            .iter()
                            .map(|item| self.catalog_row(item, selected.is_some(), cx)),
                    ),
                )
                .overflow_y_scrollbar()
                .into_any_element()
        };
        v_flex()
            .size_full()
            .child(self.catalog_header(sources.len(), cx))
            .child(self.catalog_toolbar(updates, cx))
            .children(self.catalogs.message.as_ref().map(|(error, text)| {
                h_flex()
                    .px_8()
                    .pb_3()
                    .gap_2()
                    .items_start()
                    .text_sm()
                    .text_color(if *error { theme.danger } else { theme.success })
                    .child(
                        Icon::new(if *error {
                            IconName::TriangleAlert
                        } else {
                            IconName::Check
                        })
                        .small()
                        .flex_none()
                        .mt_0p5(),
                    )
                    .child(div().flex_1().min_w_0().child(text.clone()))
            }))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(body)
                    .children(selected.map(|item| self.catalog_details(item, cx))),
            )
            .into_any_element()
    }

    fn catalog_header(&self, sources: usize, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let busy = self.catalogs.busy;
        h_flex()
            .px_8()
            .pt_6()
            .pb_4()
            .gap_4()
            .items_start()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .text_xl()
                            .font_semibold()
                            .child(page_title(self.snapshot(), &PageId::Catalog)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(page_description(self.snapshot(), &PageId::Catalog)),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .child(
                        kit::button("catalog-refresh-all")
                            .icon(Icon::new(Lucide::RefreshCw))
                            .ghost()
                            .small()
                            .loading(busy)
                            .disabled(busy || sources == 0)
                            .hint("Скачать источники заново", cx)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let sources = prefs::current(cx).catalogs.sources.clone();
                                this.refresh_catalogs(sources, cx)
                            })),
                    )
                    .child(
                        kit::button("catalog-sources")
                            .icon(Icon::new(Lucide::GitBranch))
                            .label(if sources == 0 {
                                "Источники".to_string()
                            } else {
                                format!("Источники · {sources}")
                            })
                            .outline()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_catalog_sources(window, cx)
                            })),
                    ),
            )
    }

    fn catalog_toolbar(&self, updates: usize, cx: &Context<Self>) -> impl IntoElement + use<> {
        let show = |id: &'static str, show: Show, label: String| {
            kit::button(id)
                .label(label)
                .ghost()
                .small()
                .selected(self.catalogs.show == show)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.catalogs.show = show;
                    cx.notify();
                }))
        };
        let kind = |id: ElementId, kind: Option<_>, label: &'static str| {
            kit::button(id)
                .label(label)
                .when_some(kind, |button, kind| button.icon(kind_icon(kind)))
                .ghost()
                .xsmall()
                .selected(self.catalogs.kind == kind)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.catalogs.kind = kind;
                    cx.notify();
                }))
        };
        v_flex()
            .px_8()
            .pb_3()
            .gap_2()
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.catalogs.search)
                                .small()
                                .cleanable(true)
                                .prefix(Icon::new(IconName::Search).small()),
                        ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_1()
                            .child(show("catalog-show-all", Show::All, "Все".into()))
                            .child(show(
                                "catalog-show-installed",
                                Show::Installed,
                                "Установленные".into(),
                            ))
                            .child(show(
                                "catalog-show-updates",
                                Show::Updates,
                                if updates == 0 {
                                    "Обновления".into()
                                } else {
                                    format!("Обновления · {updates}")
                                },
                            )),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .child(kind("catalog-kind-all".into(), None, "Все типы"))
                    .children(KINDS.into_iter().map(|item| {
                        kind(
                            ElementId::Name(format!("catalog-kind-{item:?}").into()),
                            Some(item),
                            kind_filter(item),
                        )
                    })),
            )
    }

    fn catalog_row(&self, item: &Item, narrow: bool, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let selected = self
            .catalogs
            .selected
            .as_ref()
            .is_some_and(|(source, id)| item.key() == (source, id.as_str()));
        let (source, entry) = (item.source.clone(), item.entry.clone());
        h_flex()
            .id(hashed_id("catalog-row", &item.key()))
            .px_8()
            .when(narrow, |row| row.pr_4())
            .py_2p5()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(theme.border.opacity(0.6))
            .cursor_pointer()
            .map(|row| {
                if selected {
                    row.bg(theme.list_active)
                } else {
                    row.hover(|row| row.bg(theme.list_hover))
                }
            })
            .child(kind_badge(item, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .min_w_0()
                                    .text_sm()
                                    .font_medium()
                                    .child(faded(item.entry.title.clone())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(item.entry.version.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(faded(format!(
                                "{} · {}",
                                item.catalog.title, item.entry.description
                            ))),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .child(self.catalog_action(item, "row", cx)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if this
                    .catalogs
                    .selected
                    .as_ref()
                    .is_some_and(|(old, id)| *old == source && *id == entry.id)
                {
                    this.deselect_catalog_entry(cx);
                } else {
                    this.select_catalog_entry(source.clone(), entry.clone(), cx);
                }
            }))
            .into_any_element()
    }

    /// The main action of an entry: install, connect again or update; an
    /// installed entry gets a mark instead.
    pub(super) fn catalog_action(
        &self,
        item: &Item,
        place: &'static str,
        cx: &Context<Self>,
    ) -> AnyElement {
        let label = match item.status {
            Status::Available => "Установить",
            Status::Detached => "Подключить",
            Status::Update => "Обновить",
            Status::Installed => {
                let theme = cx.theme();
                return h_flex()
                    .gap_1()
                    .items_center()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(
                        Icon::new(Lucide::CircleCheck)
                            .xsmall()
                            .text_color(theme.success),
                    )
                    .child("Установлено")
                    .into_any_element();
            }
        };
        let blocked = self.catalog_blocked(item.entry.kind);
        let working = self.catalogs.busy
            && self
                .catalogs
                .working
                .as_ref()
                .is_some_and(|(source, id)| item.key() == (source, id.as_str()));
        let (source, entry) = (item.source.clone(), item.entry.clone());
        kit::button(hashed_id("catalog-install", &(place, item.key())))
            .label(label)
            .small()
            .map(|button| {
                if item.status == Status::Update {
                    button.primary()
                } else {
                    button.outline()
                }
            })
            .loading(working)
            .disabled(!self.catalog_idle() || blocked.is_some())
            .when_some(blocked, |button, reason| button.hint(reason, cx))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.install_catalog_entry(source.clone(), entry.clone(), cx)
            }))
            .into_any_element()
    }

    fn catalog_empty(
        &self,
        title: &'static str,
        text: &'static str,
        add: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .justify_center()
            .gap_2()
            .p_8()
            .child(
                Icon::new(Lucide::Library)
                    .large()
                    .text_color(theme.muted_foreground),
            )
            .child(div().font_medium().child(title))
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(text),
            )
            .when(add, |empty| {
                empty.child(
                    h_flex()
                        .gap_2()
                        .mt_2()
                        .child(
                            kit::button("catalog-add-open")
                                .label("Добавить открытые каталоги")
                                .primary()
                                .small()
                                .loading(self.catalogs.busy)
                                .disabled(self.catalogs.busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let sources = OPEN.iter().map(|open| open.source()).collect();
                                    this.add_catalog_sources(sources, cx)
                                })),
                        )
                        .child(
                            kit::button("catalog-add-first")
                                .label("Источники…")
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_catalog_sources(window, cx)
                                })),
                        ),
                )
            })
            .into_any_element()
    }
}

/// Icon of the entry kind on a tinted square.
pub(super) fn kind_badge(item: &Item, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    div()
        .flex_none()
        .size(prefs::scaled(32.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.radius)
        .bg(theme.muted)
        .text_color(theme.muted_foreground)
        .child(kind_icon(item.entry.kind).small())
}
