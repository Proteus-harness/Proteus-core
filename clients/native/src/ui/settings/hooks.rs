//! Hooks page: the ordered hook chain and the hooks still available.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::*;
use gpui_kit::*;

use super::SettingsView;
use super::widgets::{card, hashed_id, hint_text, mono, note, section};
use crate::settings::catalog::{HOOK_SLOT, module_info};
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
                let header = h_flex()
                    .px_4()
                    .py_3()
                    .gap_3()
                    .items_start()
                    .child(
                        div()
                            .w(prefs::scaled(20.))
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(format!("{}.", ix + 1)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(mono(id.clone(), cx).text_sm())
                            .children(hint_text(description, cx)),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new(hashed_id("hook-up", id))
                                    .icon(IconName::ArrowUp)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Выше")
                                    .disabled(ix == 0)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_draft(cx, |draft| draft.move_hook(&up, -1))
                                    })),
                            )
                            .child(
                                Button::new(hashed_id("hook-down", id))
                                    .icon(IconName::ArrowDown)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Ниже")
                                    .disabled(ix + 1 == count)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_draft(cx, |draft| draft.move_hook(&down, 1))
                                    })),
                            )
                            .child(
                                Button::new(hashed_id("hook-off", id))
                                    .label("Отключить")
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_draft(cx, |draft| draft.set_hook(&off, false))
                                    })),
                            ),
                    );
                v_flex()
                    .child(header)
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
                h_flex()
                    .px_4()
                    .py_3()
                    .gap_3()
                    .items_center()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(mono(module.id.clone(), cx).text_sm())
                            .children(hint_text(
                                module.description.clone().unwrap_or_default(),
                                cx,
                            )),
                    )
                    .child(
                        Button::new(hashed_id("hook-on", &module.id))
                            .label("Включить")
                            .outline()
                            .xsmall()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_draft(cx, |draft| draft.set_hook(&id, true))
                            })),
                    )
                    .into_any_element()
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
