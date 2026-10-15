//! Dialog with the git repositories the catalog reads.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::Input;
use gpui_kit::component::*;
use gpui_kit::*;

use super::super::{SettingsView, widgets::hashed_id};
use crate::catalog::open::OPEN;
use crate::ui::fade::faded;
use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::prefs;

const DIALOG_WIDTH: f32 = 600.;
const PROBLEMS: usize = 3;

impl SettingsView {
    pub(super) fn open_catalog_sources(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            dialog
                .title("Источники каталога")
                .w(px(DIALOG_WIDTH))
                .children(view.upgrade().map(|view| sources(&view, cx)))
        });
    }
}

fn sources(view: &Entity<SettingsView>, cx: &App) -> AnyElement {
    let this = view.read(cx);
    let theme = cx.theme();
    let loading = &this.catalogs.loading;
    let rows =
        prefs::current(cx)
            .catalogs
            .sources
            .iter()
            .map(|source| {
                let found = this
                    .catalogs
                    .loaded
                    .iter()
                    .find(|(old, _)| old == source)
                    .map(|(_, catalog)| catalog);
                let mut about = source.url.clone();
                if let Some(branch) = &source.branch {
                    about.push_str(&format!(" · {branch}"));
                }
                let fetching = loading.contains(source);
                about.push_str(&match found {
                    _ if fetching => " · скачивается…".to_string(),
                    Some(catalog) => format!(" · записей: {}", catalog.entries.len()),
                    None => " · не загружен".to_string(),
                });
                let (update, forget) = (view.downgrade(), view.downgrade());
                let (updated, forgotten) = (source.clone(), source.clone());
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_3()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().font_medium().child(faded(found.map_or_else(
                                || source.url.clone(),
                                |catalog| catalog.title.clone(),
                            ))))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(faded(about)),
                            )
                            .children(found.into_iter().flat_map(|catalog| {
                                let more = catalog.problems.len().saturating_sub(PROBLEMS);
                                catalog
                                    .problems
                                    .iter()
                                    .take(PROBLEMS)
                                    .cloned()
                                    .chain((more > 0).then(|| format!("…и ещё {more}")))
                                    .map(|problem| {
                                        div().text_xs().text_color(theme.warning).child(problem)
                                    })
                            })),
                    )
                    .child(
                        kit::button(hashed_id("catalog-source-refresh", source))
                            .icon(Icon::new(Lucide::RefreshCw))
                            .ghost()
                            .xsmall()
                            .loading(fetching)
                            .disabled(fetching)
                            .hint("Скачать заново", cx)
                            .on_click(move |_, _, cx| {
                                update
                                    .update(cx, |this, cx| {
                                        this.refresh_catalogs(vec![updated.clone()], cx)
                                    })
                                    .ok();
                            }),
                    )
                    .child(
                        kit::button(hashed_id("catalog-source-forget", source))
                            .label("Убрать")
                            .ghost()
                            .xsmall()
                            .on_click(move |_, _, cx| {
                                forget
                                    .update(cx, |this, cx| this.forget_catalog(&forgotten, cx))
                                    .ok();
                            }),
                    )
            })
            .collect::<Vec<_>>();
    let add = view.downgrade();
    v_flex()
        .gap_3()
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Git-репозитории с proteus-catalog.json в корне или с папками Agent Skills (SKILL.md). Скачивание не запускает сборку или install-скрипты."),
        )
        .child(Input::new(&this.catalogs.url).small())
        .child(
            h_flex()
                .gap_2()
                .child(div().flex_1().child(Input::new(&this.catalogs.branch).small()))
                .child(
                    kit::button("catalog-add")
                        .label("Добавить")
                        .primary()
                        .small()
                        .on_click(move |_, window, cx| {
                            add.update(cx, |this, cx| this.add_catalog(window, cx)).ok();
                        }),
                ),
        )
        .child(if rows.is_empty() {
            div()
                .py_2()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("Источников пока нет")
                .into_any_element()
        } else {
            v_flex()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .children(rows)
                .into_any_element()
        })
        .children(open_sources(view, cx))
        .children(this.catalogs.message.as_ref().map(|(error, text)| {
            div()
                .text_xs()
                .text_color(if *error { theme.danger } else { theme.success })
                .child(text.clone())
        }))
        .into_any_element()
}

/// Public skill repositories that are not added yet.
fn open_sources(view: &Entity<SettingsView>, cx: &App) -> Option<AnyElement> {
    let theme = cx.theme();
    let added = &prefs::current(cx).catalogs.sources;
    let rows = OPEN
        .iter()
        .filter(|open| !added.contains(&open.source()))
        .map(|open| {
            let view = view.downgrade();
            let source = open.source();
            h_flex()
                .px_3()
                .py_2()
                .gap_3()
                .items_center()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_baseline()
                                .child(div().text_sm().font_medium().child(open.title))
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(faded(source.name())),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(faded(open.about)),
                        ),
                )
                .child(
                    kit::button(hashed_id("catalog-open-add", &source))
                        .label("Добавить")
                        .ghost()
                        .xsmall()
                        .on_click(move |_, _, cx| {
                            view.update(cx, |this, cx| {
                                this.add_catalog_sources(vec![source.clone()], cx)
                            })
                            .ok();
                        }),
                )
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return None;
    }
    Some(
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .font_medium()
                    .text_color(theme.muted_foreground)
                    .child("Открытые каталоги skills"),
            )
            .child(
                v_flex()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .children(rows),
            )
            .into_any_element(),
    )
}
