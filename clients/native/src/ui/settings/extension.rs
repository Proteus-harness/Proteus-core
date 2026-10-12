//! Page of one extension: what it adds, where its icon lives and where it
//! comes from.

use crate::ui::kit;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::*;
use gpui_kit::*;

use super::SettingsView;
use super::widgets::{MenuOption, card, mono, note, row, section, select_menu};
use crate::preferences::BadgePlace;
use crate::settings::catalog::PageId;
use crate::ui::extensions::{self, Manifest};
use crate::ui::prefs;

const BADGE_PLACES: [(BadgePlace, &str); 7] = [
    (BadgePlace::ComposerStart, "Под полем ввода, слева"),
    (BadgePlace::ComposerEnd, "Под полем ввода, справа"),
    (BadgePlace::HeaderStart, "В верхней полосе, слева"),
    (BadgePlace::HeaderEnd, "В верхней полосе, справа"),
    (BadgePlace::StatusStart, "В строке состояния, слева"),
    (BadgePlace::StatusEnd, "В строке состояния, справа"),
    (BadgePlace::Hidden, "Скрыт"),
];

impl SettingsView {
    /// Switch beside the page title; required pages have none.
    pub(super) fn extension_switch(&self, id: &str, cx: &App) -> Option<AnyElement> {
        let manifest = extensions::find(id)?;
        let id = manifest.id;
        Some(
            kit::switch("extension-page-enabled")
                .checked(extensions::is_enabled(id, cx))
                .tooltip(format!("Включить: {}", manifest.title))
                .on_click(move |checked: &bool, _, cx| {
                    let checked = *checked;
                    prefs::update(cx, |prefs| prefs.extensions.set_enabled(id, checked));
                })
                .into_any_element(),
        )
    }

    pub(super) fn extension_page(&self, id: &str, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(manifest) = extensions::find(id) else {
            return vec![
                card(
                    [
                        note(format!("Расширения `{id}` больше нет."), cx).into_any_element(),
                        div()
                            .px_4()
                            .pb_3()
                            .child(
                                kit::button("extension-back")
                                    .label("К списку расширений")
                                    .outline()
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.select_page(PageId::Extensions, cx)
                                    })),
                            )
                            .into_any_element(),
                    ],
                    cx,
                )
                .into_any_element(),
            ];
        };
        let enabled = extensions::is_enabled(manifest.id, cx);
        let mut parts = Vec::new();
        if !enabled {
            parts.push(
                card(
                    [note(
                        "Расширение выключено. Включите его переключателем рядом с заголовком.",
                        cx,
                    )
                    .into_any_element()],
                    cx,
                )
                .into_any_element(),
            );
        }
        parts.push(
            section(
                "Что добавляет",
                None,
                None,
                card(self.surface_rows(manifest, enabled, cx), cx),
                cx,
            )
            .into_any_element(),
        );
        parts.push(self.origin_section(manifest, cx).into_any_element());
        parts
    }

    fn surface_rows(
        &self,
        manifest: &'static Manifest,
        enabled: bool,
        cx: &App,
    ) -> Vec<AnyElement> {
        let id = manifest.id;
        let mut rows = Vec::new();
        if let Some(panel) = &manifest.panel {
            rows.push(
                row(
                    format!("Панель «{}»", panel.title),
                    Some("Открывается вкладкой рядом с чатами.".into()),
                    div(),
                    false,
                    cx,
                )
                .into_any_element(),
            );
        }
        if manifest.document.is_some() {
            rows.push(
                row(
                    "Вкладки",
                    Some("Открывает свои документы во вкладках.".into()),
                    div(),
                    false,
                    cx,
                )
                .into_any_element(),
            );
        }
        if manifest.badge.is_some() {
            rows.push(
                row(
                    "Значок",
                    Some(
                        "Его можно перетащить мышью между полем ввода, верхней полосой и \
                         строкой состояния."
                            .into(),
                    ),
                    if enabled {
                        badge_select(manifest, cx)
                    } else {
                        div().into_any_element()
                    },
                    false,
                    cx,
                )
                .into_any_element(),
            );
            rows.push(
                row(
                    "Закрепить значок",
                    Some("Закреплённый значок не перетаскивается.".into()),
                    kit::switch("extension-page-pinned")
                        .checked(prefs::current(cx).extensions.is_pinned(id))
                        .disabled(!enabled)
                        .on_click(move |checked: &bool, _, cx| {
                            let checked = *checked;
                            prefs::update(cx, |prefs| prefs.extensions.set_pinned(id, checked));
                        }),
                    false,
                    cx,
                )
                .into_any_element(),
            );
        }
        if manifest.page.is_some() {
            rows.push(
                row(
                    "Страница",
                    Some("Показывается здесь, пока расширение включено.".into()),
                    div(),
                    false,
                    cx,
                )
                .into_any_element(),
            );
        }
        if rows.is_empty() {
            rows.push(note("У расширения нет поверхностей.", cx).into_any_element());
        }
        rows
    }

    fn origin_section(&self, manifest: &'static Manifest, cx: &Context<Self>) -> Div {
        let id = manifest.id;
        let rows = match extensions::folder(id) {
            None => {
                vec![note("Встроено в клиент; его можно только выключить.", cx).into_any_element()]
            }
            Some(folder) => {
                let confirming = self.removing.as_deref() == Some(id);
                let control =
                    if confirming {
                        h_flex()
                            .gap_2()
                            .child(
                                kit::button("extension-page-remove-yes")
                                    .label("Удалить")
                                    .danger()
                                    .small()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.remove_extension(id, cx)
                                    })),
                            )
                            .child(
                                kit::button("extension-page-remove-no")
                                    .label("Отмена")
                                    .ghost()
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.removing = None;
                                        cx.notify();
                                    })),
                            )
                    } else {
                        h_flex().child(
                            kit::button("extension-page-remove")
                                .label("Удалить…")
                                .outline()
                                .small()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.removing = Some(id.to_string());
                                    cx.notify();
                                })),
                        )
                    };
                vec![
                    row("Установлено из папки", None, control, false, cx).into_any_element(),
                    div()
                        .px_4()
                        .pb_3()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(mono(folder.display().to_string(), cx))
                        .into_any_element(),
                ]
            }
        };
        section("Источник", None, None, card(rows, cx), cx)
    }
}

/// Where the extension icon lives; it can also be dragged or hidden from
/// its context menu.
fn badge_select(manifest: &'static Manifest, cx: &App) -> AnyElement {
    let current = extensions::badge_place(manifest, cx);
    let label = BADGE_PLACES
        .iter()
        .find(|(place, _)| *place == current)
        .map_or("Скрыт", |(_, label)| label);
    let options = BADGE_PLACES
        .iter()
        .map(|(place, label)| MenuOption {
            label: (*label).into(),
            value: *place,
            checked: *place == current,
        })
        .collect();
    let id = manifest.id;
    select_menu(
        "extension-page-badge".into(),
        label,
        options,
        move |place, cx| prefs::update(cx, |prefs| prefs.extensions.set_badge_place(id, place)),
    )
}
