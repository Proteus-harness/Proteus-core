//! Hooks page: the ordered hook chain and the hooks still available.

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::*;
use gpui_kit::*;

use super::SettingsView;
use super::widgets::{card, hashed_id, item_row, named, note, section};
use crate::settings::catalog::{HOOK_SLOT, module_info};
use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::prefs;

impl SettingsView {
    pub(super) fn hooks_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(snapshot) = self.snapshot() else {
            return Vec::new();
        };
        let theme = cx.theme();
        let count = self.draft.hooks.len();
        let enabled: Vec<AnyElement> = self
            .draft
            .hooks
            .iter()
            .enumerate()
            .map(|(ix, id)| {
                let description = module_info(snapshot, HOOK_SLOT, id)
                    .map(|info| info.description.unwrap_or_default().to_string())
                    .unwrap_or_else(|| "Модуль не найден в текущей сборке".to_string());
                let key = format!("params:{HOOK_SLOT}:{id}");
                let open = self.is_open(&key);
                let (up, down, off) = (id.clone(), id.clone(), id.clone());
                let title = h_flex()
                    .min_w_0()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .w(prefs::scaled(20.))
                            .flex_none()
                            .text_color(theme.muted_foreground)
                            .child(format!("{}.", ix + 1)),
                    )
                    .child(named(
                        hashed_id("hook-name", id),
                        id.clone(),
                        Some(description.into()),
                        None,
                        cx,
                    ));
                let header = item_row(
                    title,
                    [
                        kit::button(hashed_id("hook-up", id))
                            .icon(IconName::ArrowUp)
                            .ghost()
                            .xsmall()
                            .hint("Выше", cx)
                            .disabled(ix == 0)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_draft(cx, |draft| draft.move_hook(&up, -1))
                            }))
                            .into_any_element(),
                        kit::button(hashed_id("hook-down", id))
                            .icon(IconName::ArrowDown)
                            .ghost()
                            .xsmall()
                            .hint("Ниже", cx)
                            .disabled(ix + 1 == count)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_draft(cx, |draft| draft.move_hook(&down, 1))
                            }))
                            .into_any_element(),
                        kit::button(hashed_id("hook-off", id))
                            .label("Отключить")
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_draft(cx, |draft| draft.set_hook(&off, false))
                            }))
                            .into_any_element(),
                    ],
                );
                v_flex()
                    .child(self.spotted(&super::spot::hook(id), header, cx))
                    .child(self.params_disclosure(HOOK_SLOT, id, &key, open, cx))
                    .into_any_element()
            })
            .collect();
        let enabled = if enabled.is_empty() {
            card(
                [note("Обработчики не включены.", cx).into_any_element()],
                cx,
            )
        } else {
            card(enabled, cx)
        };

        let available: Vec<AnyElement> = snapshot
            .hook_modules
            .iter()
            .filter(|module| !self.draft.hooks.contains(&module.id))
            .map(|module| {
                let id = module.id.clone();
                let row = item_row(
                    named(
                        hashed_id("hook-name", &module.id),
                        module.id.clone(),
                        module.description.clone().map(Into::into),
                        None,
                        cx,
                    ),
                    [kit::button(hashed_id("hook-on", &module.id))
                        .label("Включить")
                        .outline()
                        .xsmall()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_draft(cx, |draft| draft.set_hook(&id, true))
                        }))
                        .into_any_element()],
                );
                self.spotted(&super::spot::hook(&module.id), row, cx)
            })
            .collect();
        let available = if available.is_empty() {
            let text = if snapshot.hook_modules.is_empty() {
                "В профиле нет модулей обработчиков."
            } else {
                "Все обработчики включены."
            };
            card([note(text, cx).into_any_element()], cx)
        } else {
            card(available, cx)
        };
        vec![
            section("Включены", None, None, enabled, cx).into_any_element(),
            section("Доступны", None, None, available, cx).into_any_element(),
        ]
    }
}
