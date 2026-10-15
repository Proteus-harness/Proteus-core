//! Details of the selected catalog entry beside the list.

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::text::TextView;
use gpui_kit::component::*;
use gpui_kit::*;

use super::super::{
    SettingsView,
    widgets::{Tone, chip, hashed_id},
};
use super::items::{Item, Status};
use super::view::kind_badge;
use crate::catalog::schema::Kind;
use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::prefs;

const DETAILS_WIDTH: f32 = 420.;

impl SettingsView {
    pub(super) fn catalog_details(&self, item: &Item, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let entry = item.entry;
        let mut facts = vec![
            chip(entry.kind.title(), Tone::Muted, cx),
            chip(entry.version.clone(), Tone::Muted, cx),
        ];
        if let Some(category) = item.category() {
            facts.push(chip(category.to_string(), Tone::Muted, cx));
        }
        match (item.status, item.installed) {
            (Status::Update, Some(old)) => facts.push(chip(
                format!("установлена {}", old.version),
                Tone::Warning,
                cx,
            )),
            (Status::Detached, _) => facts.push(chip("не подключено", Tone::Warning, cx)),
            (Status::Installed, _) => facts.push(chip("установлено", Tone::Success, cx)),
            _ => {}
        }
        let notes = [
            match entry.kind {
                Kind::McpServer => Some("Сервер записывается в профиль сразу при установке."),
                Kind::AgentPlugin => Some(
                    "Пакет подключается к профилю сразу при установке. Его файлы остаются на диске после удаления.",
                ),
                Kind::ClientExtension | Kind::Skill => None,
            },
            self.catalog_blocked(entry.kind),
        ];
        let removable = item
            .installed
            .filter(|_| item.status != Status::Available)
            .cloned();
        div()
            .id(hashed_id("catalog-details", &item.key()))
            .w(prefs::scaled(DETAILS_WIDTH))
            .flex_none()
            .h_full()
            .border_l_1()
            .border_color(theme.border)
            .child(
                v_flex()
                    .px_6()
                    .py_5()
                    .gap_4()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_start()
                            .child(kind_badge(item, cx))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div().text_lg().font_semibold().child(entry.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .font_family(theme.mono_font_family.clone())
                                            .child(entry.id.clone()),
                                    ),
                            )
                            .child(
                                kit::button("catalog-details-close")
                                    .icon(IconName::Close)
                                    .ghost()
                                    .xsmall()
                                    .hint("Закрыть", cx)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.deselect_catalog_entry(cx)
                                    })),
                            ),
                    )
                    .child(h_flex().flex_wrap().gap_1p5().children(facts))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(self.catalog_action(item, "details", cx))
                            .children(removable.map(|installed| {
                                kit::button(hashed_id("catalog-remove", &item.key()))
                                    .label("Удалить")
                                    .ghost()
                                    .small()
                                    .disabled(
                                        !self.catalog_idle()
                                            || self.catalog_blocked(installed.kind).is_some(),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.remove_catalog_entry(installed.clone(), cx)
                                    }))
                            })),
                    )
                    .child(div().text_sm().child(entry.description.clone()))
                    .children(notes.into_iter().flatten().map(|note| {
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(note)
                    }))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("Источник: {}", item.catalog.title)),
                    )
                    .children(self.catalogs.readme.as_ref().map(|state| {
                        div()
                            .pt_4()
                            .border_t_1()
                            .border_color(theme.border)
                            .child(TextView::new(state))
                    })),
            )
            .overflow_y_scrollbar()
            .into_any_element()
    }
}
