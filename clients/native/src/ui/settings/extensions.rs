//! Client extensions: switch them on and off and order the panel widgets.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::*;
use gpui_kit::*;

use super::SettingsView;
use super::widgets::{Tone, card, chip, note, row, section};
use crate::ui::extensions::{self, Manifest};
use crate::ui::prefs;

impl SettingsView {
    pub(super) fn extensions_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let extensions = &prefs::current(cx).extensions;
        let installed = extensions::CATALOG
            .iter()
            .enumerate()
            .map(|(ix, manifest)| {
                extension_row(ix, manifest, extensions.is_enabled(manifest.id), cx)
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let panel_ids = extensions::panel_ids();
        let visible: Vec<&'static str> = extensions
            .ordered(&panel_ids)
            .into_iter()
            .filter(|id| extensions.is_enabled(id))
            .collect();
        let order = if visible.is_empty() {
            card(
                [note("Все виджеты панели выключены.", cx).into_any_element()],
                cx,
            )
        } else {
            card(
                visible
                    .iter()
                    .enumerate()
                    .map(|(ix, id)| order_row(&visible, ix, id, cx).into_any_element()),
                cx,
            )
        };

        let mut parts = self.preferences_problem(cx);
        parts
            .push(section("Установленные", None, None, card(installed, cx), cx).into_any_element());
        parts.push(
            section(
                "Порядок в панели",
                Some("Виджеты также можно перетаскивать за заголовок прямо в панели.".into()),
                None,
                order,
                cx,
            )
            .into_any_element(),
        );
        parts
    }
}

fn extension_row(ix: usize, manifest: &'static Manifest, enabled: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let id = manifest.id;
    let title = h_flex()
        .gap_2()
        .flex_wrap()
        .child((manifest.icon)().small().text_color(theme.muted_foreground))
        .child(div().text_sm().child(manifest.title))
        .children(
            manifest
                .panel
                .as_ref()
                .map(|panel| chip(format!("Панель · {}", panel.title), Tone::Muted, cx)),
        )
        .children(
            manifest
                .page
                .as_ref()
                .map(|page| chip(format!("Страница · {}", page.title), Tone::Muted, cx)),
        );
    row(
        title,
        Some(manifest.description.into()),
        Switch::new(("extension-enabled", ix))
            .checked(enabled)
            .on_click(move |checked: &bool, _, cx| {
                let checked = *checked;
                prefs::update(cx, |prefs| prefs.extensions.set_enabled(id, checked));
            }),
        false,
        cx,
    )
}

/// One widget in panel order. Moves are relative to the visible widgets, so
/// switched-off ones never swallow a click.
fn order_row(visible: &[&'static str], ix: usize, id: &'static str, cx: &App) -> Div {
    let theme = cx.theme();
    let Some(manifest) = extensions::find(id) else {
        return div();
    };
    let title = manifest
        .panel
        .as_ref()
        .map_or(manifest.title, |panel| panel.title);
    let up = (ix > 0).then(|| visible[ix - 1]);
    let down = (ix + 1 < visible.len()).then(|| visible.get(ix + 2).copied());
    h_flex()
        .px_4()
        .py_2()
        .gap_3()
        .child(
            div()
                .w(px(20.))
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(format!("{}.", ix + 1)),
        )
        .child((manifest.icon)().small().text_color(theme.muted_foreground))
        .child(div().flex_1().min_w_0().truncate().text_sm().child(title))
        .child(
            Button::new(("extension-up", ix))
                .ghost()
                .xsmall()
                .icon(IconName::ArrowUp)
                .tooltip("Выше")
                .disabled(up.is_none())
                .on_click(move |_, _, cx| {
                    if let Some(before) = up {
                        move_widget(id, Some(before), cx);
                    }
                }),
        )
        .child(
            Button::new(("extension-down", ix))
                .ghost()
                .xsmall()
                .icon(IconName::ArrowDown)
                .tooltip("Ниже")
                .disabled(down.is_none())
                .on_click(move |_, _, cx| {
                    if let Some(before) = down {
                        move_widget(id, before, cx);
                    }
                }),
        )
}

fn move_widget(id: &'static str, before: Option<&'static str>, cx: &mut App) {
    let known = extensions::panel_ids();
    prefs::update(cx, |prefs| prefs.extensions.move_before(&known, id, before));
}
