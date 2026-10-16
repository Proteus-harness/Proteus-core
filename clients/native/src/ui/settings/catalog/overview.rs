//! Start screen of the catalog: updates and installed entries first, then
//! the types and the sources with their categories to narrow the list by.
//! The whole list only opens once a search, a filter or a scope asks for it.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::super::{SettingsView, widgets::hashed_id};
use super::items::{Item, KINDS, OVERVIEW_ROWS, Status, kind_filter, kind_icon, overview_rows};
use super::{Scope, Show};
use crate::catalog::{Source, schema::Catalog};
use crate::settings::catalog::plural;
use crate::ui::fade::faded;
use crate::ui::kit::{self, Pressable as _};

impl SettingsView {
    pub(super) fn catalog_overview(
        &self,
        items: &[Item],
        narrow: bool,
        width: f32,
        cx: &Context<Self>,
    ) -> AnyElement {
        let (updates, installed) = overview_rows(items);
        let count = |status| items.iter().filter(|item| item.status == status).count();
        let profile = self.profile_blocked();
        let rows = |rows: Vec<&Item>| {
            rows.into_iter()
                .map(|item| self.catalog_row(item, narrow, profile, cx))
                .collect::<Vec<_>>()
        };
        let mut sections = Vec::new();
        if !updates.is_empty() {
            sections.push(
                section("Обновления", count(Status::Update), Show::Updates, cx)
                    .children(rows(updates))
                    .into_any_element(),
            );
        }
        if !installed.is_empty() {
            sections.push(
                section(
                    "Установленные",
                    count(Status::Installed),
                    Show::Installed,
                    cx,
                )
                .children(rows(installed))
                .into_any_element(),
            );
        }
        if sections.is_empty() {
            sections.push(
                div()
                    .px_8()
                    .pt_5()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        "Ничего не установлено. Найдите запись поиском или выберите тип или источник.",
                    )
                    .into_any_element(),
            );
        }
        // The page padding takes its share before the columns.
        let columns = |least: f32, most: u16| ((width - 64.) / least).clamp(1., most as f32) as u16;
        sections.extend(self.overview_kinds(items, columns(200., 4), cx));
        sections.push(
            self.overview_sources(items, columns(320., 3), cx)
                .into_any_element(),
        );
        div()
            .id("catalog-overview")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                v_flex()
                    .id("catalog-overview-sections")
                    .size_full()
                    .pb_6()
                    .overflow_y_scroll()
                    .track_scroll(&self.catalogs.overview)
                    .children(sections),
            )
            .vertical_scrollbar(&self.catalogs.overview)
            .into_any_element()
    }

    fn overview_kinds(
        &self,
        items: &[Item],
        columns: u16,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let kinds = KINDS
            .into_iter()
            .map(|kind| {
                let count = items.iter().filter(|item| item.entry.kind == kind).count();
                (kind, count)
            })
            .filter(|(_, count)| *count > 0)
            .collect::<Vec<_>>();
        // A single type narrows nothing; «Все» already lists it.
        if kinds.len() < 2 {
            return None;
        }
        let cards = kinds.into_iter().map(|(kind, count)| {
            card(
                ElementId::Name(format!("catalog-overview-kind-{kind:?}").into()),
                cx,
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(kind_icon(kind).small())
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().font_medium().child(kind_filter(kind)))
                            .child(muted(plural(count, "запись", "записи", "записей"), cx)),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.catalogs.kind = Some(kind);
                cx.notify();
            }))
        });
        Some(
            v_flex()
                .child(heading("Типы", cx))
                .child(
                    div()
                        .px_8()
                        .grid()
                        .grid_cols(columns)
                        .gap_2()
                        .children(cards),
                )
                .into_any_element(),
        )
    }

    fn overview_sources(
        &self,
        items: &[Item],
        columns: u16,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let cards =
            self.catalogs.loaded.iter().map(|(source, catalog)| {
                let count = plural(
                    items.iter().filter(|item| item.source == source).count(),
                    "запись",
                    "записи",
                    "записей",
                );
                let name = source.name();
                // A repository without an index is titled by its name already.
                let about = if catalog.title == name {
                    count
                } else {
                    format!("{name} · {count}")
                };
                let categories = categories(catalog);
                let scope = |category: Option<String>| Scope {
                    source: source.clone(),
                    category,
                };
                let whole = scope(None);
                card(hashed_id("catalog-overview-source", &source.key()), cx)
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(Icon::new(Lucide::GitBranch).small())
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_medium()
                                            .child(faded(catalog.title.clone())),
                                    )
                                    .child(muted(about, cx)),
                            ),
                    )
                    .when(categories.len() > 1, |card| {
                        card.child(h_flex().flex_wrap().gap_1().children(
                            categories.into_iter().map(|(id, title, count)| {
                                let scope = scope(Some(id.clone()));
                                kit::button(hashed_id(
                                    "catalog-overview-category",
                                    &(source.key(), &id),
                                ))
                                .label(format!("{title} · {count}"))
                                .outline()
                                .xsmall()
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.catalogs.scope = Some(scope.clone());
                                        cx.notify();
                                    },
                                ))
                            }),
                        ))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.catalogs.scope = Some(whole.clone());
                        cx.notify();
                    }))
            });
        v_flex().child(heading("Источники", cx)).child(
            div()
                .px_8()
                .grid()
                .grid_cols(columns)
                .gap_3()
                .children(cards),
        )
    }

    /// Title of the scope for its chip in the toolbar.
    pub(super) fn catalog_scope_title(&self, scope: &Scope) -> String {
        let Some((_, catalog)) = self
            .catalogs
            .loaded
            .iter()
            .find(|(source, _)| *source == scope.source)
        else {
            return scope.source.name();
        };
        match &scope.category {
            None => catalog.title.clone(),
            Some(id) => format!("{} · {}", catalog.title, category_title(catalog, id)),
        }
    }

    /// Esc steps back from the list to the overview: the scope, the type
    /// and the search go first, then the list itself.
    pub(in crate::ui::settings) fn catalog_back(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let catalogs = &mut self.catalogs;
        if catalogs.scope.take().is_none() && catalogs.kind.take().is_none() {
            if !catalogs.query.is_empty() {
                catalogs.query.clear();
                catalogs
                    .search
                    .update(cx, |search, cx| search.set_value("", window, cx));
            } else if catalogs.show != Show::Overview {
                catalogs.show = Show::Overview;
            } else {
                return false;
            }
        }
        cx.notify();
        true
    }

    /// Drops a scope whose source is gone.
    pub(super) fn forget_catalog_scope(&mut self, source: &Source) {
        if self
            .catalogs
            .scope
            .as_ref()
            .is_some_and(|scope| scope.source == *source)
        {
            self.catalogs.scope = None;
        }
    }
}

/// Categories of a catalog in its own order with their entry counts.
fn categories(catalog: &Catalog) -> Vec<(String, String, usize)> {
    let mut found: Vec<(String, String, usize)> = Vec::new();
    for entry in &catalog.entries {
        match found.iter_mut().find(|(id, ..)| *id == entry.category) {
            Some((.., count)) => *count += 1,
            None => found.push((
                entry.category.clone(),
                category_title(catalog, &entry.category),
                1,
            )),
        }
    }
    let order = |id: &str| {
        catalog
            .categories
            .iter()
            .position(|category| category.id == id)
            .unwrap_or(usize::MAX)
    };
    found.sort_by_key(|(id, ..)| order(id));
    found
}

fn category_title(catalog: &Catalog, id: &str) -> String {
    catalog
        .categories
        .iter()
        .find(|category| category.id == id)
        .map_or_else(|| id.to_string(), |category| category.title.clone())
}

fn heading(title: &'static str, cx: &App) -> impl IntoElement + use<> {
    div()
        .px_8()
        .pt_5()
        .pb_2()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(title)
}

/// Heading with a count and a link to the full list when it does not fit.
fn section(title: &'static str, count: usize, show: Show, cx: &Context<SettingsView>) -> Div {
    let theme = cx.theme();
    v_flex().child(
        h_flex()
            .px_8()
            .pt_5()
            .pb_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border.opacity(0.6))
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .font_semibold()
                    .text_color(theme.muted_foreground)
                    .child(format!("{title} · {count}")),
            )
            .when(count > OVERVIEW_ROWS, |row| {
                row.child(
                    kit::button(hashed_id("catalog-overview-more", &title))
                        .label("Показать все")
                        .ghost()
                        .xsmall()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.catalogs.show = show;
                            cx.notify();
                        })),
                )
            }),
    )
}

fn card(id: ElementId, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    v_flex()
        .id(id)
        .min_w_0()
        .px_3()
        .py_2p5()
        .gap_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .cursor_pointer()
        .hover(|card| card.bg(theme.list_hover))
        .pressable(cx)
}

fn muted(text: String, cx: &App) -> impl IntoElement + use<> {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(faded(text))
}
