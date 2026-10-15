//! Dialog with the git repositories the catalog reads.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::Input;
use gpui_kit::component::*;
use gpui_kit::*;

use super::super::{SettingsView, widgets::hashed_id};
use crate::ui::fade::faded;
use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::prefs;

const DIALOG_WIDTH: f32 = 600.;

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
    let busy = this.catalogs.busy;
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
                about.push_str(&match found {
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
                                catalog.problems.iter().map(|problem| {
                                    div()
                                        .text_xs()
                                        .text_color(theme.warning)
                                        .child(problem.clone())
                                })
                            })),
                    )
                    .child(
                        kit::button(hashed_id("catalog-source-refresh", source))
                            .icon(Icon::new(Lucide::RefreshCw))
                            .ghost()
                            .xsmall()
                            .disabled(busy)
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
                            .disabled(busy)
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
                .child("Git-репозитории с proteus-catalog.json в корне. Скачивание не запускает сборку или install-скрипты."),
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
                        .loading(busy)
                        .disabled(busy)
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
        .children(this.catalogs.message.as_ref().map(|(error, text)| {
            div()
                .text_xs()
                .text_color(if *error { theme.danger } else { theme.success })
                .child(text.clone())
        }))
        .into_any_element()
}
