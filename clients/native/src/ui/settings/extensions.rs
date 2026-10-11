//! Client extensions: switch them on and off and choose where their icons live.

use gpui_kit::component::switch::Switch;
use gpui_kit::component::*;
use gpui_kit::*;

use super::SettingsView;
use super::widgets::{MenuOption, Tone, card, chip, mono, note, row, section, select_menu};
use crate::preferences::BadgePlace;
use crate::ui::extensions::{self, Manifest};
use crate::ui::prefs;

const BADGE_PLACES: [(BadgePlace, &str); 3] = [
    (BadgePlace::Composer, "Под полем ввода"),
    (BadgePlace::Header, "В панели вкладок"),
    (BadgePlace::Hidden, "Скрыт"),
];

impl SettingsView {
    pub(super) fn extensions_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let installed = extensions::catalog()
            .iter()
            .enumerate()
            .map(|(ix, manifest)| extension_row(ix, manifest, cx).into_any_element())
            .collect::<Vec<_>>();
        let mut parts = self.preferences_problem(cx);
        parts.push(
            section(
                "Установленные",
                Some(
                    "Значки можно перетаскивать между полем ввода и панелью вкладок. \
                     Правый клик по значку закрепляет его на месте."
                        .into(),
                ),
                None,
                card(installed, cx),
                cx,
            )
            .into_any_element(),
        );
        parts.push(installed_folder(cx).into_any_element());
        parts
    }
}

/// Where folder extensions live and which of them did not load.
fn installed_folder(cx: &App) -> Div {
    let folder = crate::declarative::manifest::root().map_or_else(
        || "папка настроек не найдена".to_string(),
        |root| root.display().to_string(),
    );
    let mut rows = vec![
        note(
            "Расширение — папка с extension.json и программой, которая рисует свои \
             панели виджетами. Изменения подхватываются при следующем запуске клиента.",
            cx,
        )
        .into_any_element(),
        div()
            .px_4()
            .pb_3()
            .child(mono(folder, cx))
            .into_any_element(),
    ];
    rows.extend(extensions::problems().iter().map(|problem| {
        v_flex()
            .px_4()
            .py_3()
            .gap_1()
            .child(chip(problem.dir.display().to_string(), Tone::Danger, cx))
            .child(div().text_sm().child(problem.error.clone()))
            .into_any_element()
    }));
    section("Из папки", None, None, card(rows, cx), cx)
}

fn extension_row(ix: usize, manifest: &'static Manifest, cx: &App) -> Div {
    let theme = cx.theme();
    let extensions = &prefs::current(cx).extensions;
    let id = manifest.id;
    let enabled = extensions.is_enabled(id);
    let title = h_flex()
        .gap_2()
        .flex_wrap()
        .child(
            Icon::new(manifest.icon)
                .small()
                .text_color(theme.muted_foreground),
        )
        .child(div().text_sm().child(manifest.title))
        .children(
            manifest
                .panel
                .as_ref()
                .map(|panel| chip(format!("Панель · {}", panel.title), Tone::Muted, cx)),
        )
        .children(
            manifest
                .document
                .as_ref()
                .map(|_| chip("Вкладки", Tone::Muted, cx)),
        )
        .children(
            (manifest.badge.is_some() && extensions.is_pinned(id))
                .then(|| chip("значок закреплён", Tone::Accent, cx)),
        );
    let control = h_flex()
        .gap_3()
        .children(
            manifest
                .badge
                .as_ref()
                .filter(|_| enabled)
                .map(|_| badge_select(ix, manifest, cx)),
        )
        .child(
            Switch::new(("extension-enabled", ix))
                .checked(enabled)
                .on_click(move |checked: &bool, _, cx| {
                    let checked = *checked;
                    prefs::update(cx, |prefs| prefs.extensions.set_enabled(id, checked));
                }),
        );
    row(title, Some(manifest.description.into()), control, false, cx)
}

/// Where the extension icon lives; it can also be dragged or hidden from
/// its context menu.
fn badge_select(ix: usize, manifest: &'static Manifest, cx: &App) -> AnyElement {
    let current = extensions::badge_place(manifest, cx);
    let label = BADGE_PLACES
        .iter()
        .find(|(place, _)| *place == current)
        .map_or("Скрыт", |(_, label)| label);
    let options = BADGE_PLACES
        .iter()
        .map(|(place, label)| MenuOption {
            label: format!("Значок: {}", label.to_lowercase()).into(),
            value: *place,
            checked: *place == current,
        })
        .collect();
    let id = manifest.id;
    select_menu(
        ElementId::NamedInteger("extension-badge".into(), ix as u64),
        format!("Значок: {}", label.to_lowercase()),
        options,
        move |place, cx| prefs::update(cx, |prefs| prefs.extensions.set_badge_place(id, place)),
    )
}
