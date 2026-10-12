//! Pages of select-one slots: implementation choice and its parameters.

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::*;
use gpui_kit::*;

use super::SettingsView;
use super::widgets::{Tone, card, chip, hashed_id, mono, note, radio_card, section};
use crate::settings::catalog::{
    CONTEXT_PROVIDER_SLOT, MODEL_SLOT, POLICY_SLOT, PageId, exports, find_slot, model_module,
    permission_description, selected_module,
};
use crate::ui::kit;

impl SettingsView {
    pub(super) fn model_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(snapshot) = self.snapshot() else {
            return Vec::new();
        };
        let view = cx.weak_entity();
        let cards: Vec<AnyElement> = snapshot
            .providers
            .iter()
            .map(|provider| {
                let title = if provider.label.is_empty() {
                    provider.model.clone()
                } else {
                    provider.label.clone()
                };
                let hint = (title != provider.model).then(|| provider.model.clone().into());
                let mut chips = vec![chip(provider.id.clone(), Tone::Muted, cx)];
                if provider.provider != provider.id {
                    chips.push(chip(provider.provider.clone(), Tone::Muted, cx));
                }
                let (view, id) = (view.clone(), provider.id.clone());
                radio_card(
                    hashed_id("provider", &provider.id),
                    self.draft.provider.as_deref() == Some(provider.id.as_str()),
                    title,
                    hint,
                    chips,
                    move |cx| {
                        let id = id.clone();
                        let _ = view.update(cx, |this, cx| {
                            this.update_draft(cx, |draft| draft.provider = Some(id))
                        });
                    },
                    cx,
                )
                .into_any_element()
            })
            .collect();
        let providers = if cards.is_empty() {
            card(
                [note("В профиле нет настроенных провайдеров моделей.", cx).into_any_element()],
                cx,
            )
        } else {
            v_flex().gap_2().children(cards)
        };
        let mut parts = vec![section("Провайдер", None, None, providers, cx).into_any_element()];
        if let Some(module) = model_module(snapshot, &self.draft) {
            parts.push(self.params_section(MODEL_SLOT, &module, cx));
        }
        parts
    }

    pub(super) fn slot_page(&self, slot: &str, cx: &Context<Self>) -> Vec<AnyElement> {
        let mut parts = Vec::new();
        if slot == POLICY_SLOT {
            parts.push(self.mode_section(cx));
        }
        parts.extend(self.implementation(slot, cx));
        parts
    }

    fn mode_section(&self, cx: &Context<Self>) -> AnyElement {
        let Some(snapshot) = self.snapshot() else {
            return div().into_any_element();
        };
        let view = cx.weak_entity();
        let cards = snapshot.permission_modes.iter().map(|mode| {
            let description = permission_description(mode);
            let (view, value) = (view.clone(), mode.clone());
            radio_card(
                hashed_id("mode", mode),
                self.draft.mode == *mode,
                mode.clone(),
                Some(description.into()),
                Vec::new(),
                move |cx| {
                    let value = value.clone();
                    let _ = view.update(cx, |this, cx| {
                        this.update_draft(cx, |draft| draft.mode = value)
                    });
                },
                cx,
            )
        });
        section(
            "Режим прав по умолчанию",
            None,
            None,
            v_flex().gap_2().children(cards),
            cx,
        )
        .into_any_element()
    }

    fn implementation(&self, slot: &str, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(snapshot) = self.snapshot() else {
            return Vec::new();
        };
        let Some(info) = find_slot(snapshot, slot) else {
            return vec![
                section(
                    "Реализация",
                    None,
                    None,
                    card(
                        [note("Этот слот недоступен в текущей сборке.", cx).into_any_element()],
                        cx,
                    ),
                    cx,
                )
                .into_any_element(),
            ];
        };
        if info.modules.is_empty() {
            return vec![
                section(
                    "Реализация",
                    None,
                    None,
                    card(
                        [note(
                            "Профиль не подключает модулей для этого слота. Реализация появится здесь, когда компонент профиля предоставит такой export.",
                            cx,
                        )
                        .into_any_element()],
                        cx,
                    ),
                    cx,
                )
                .into_any_element(),
            ];
        }
        let selected = selected_module(snapshot, &self.draft, slot).map(str::to_string);
        let view = cx.weak_entity();
        let cards = info.modules.iter().map(|module| {
            let mut chips = Vec::new();
            for label in [&module.source, &module.version] {
                if !label.is_empty() && !chips.iter().any(|(text, _)| text == label) {
                    chips.push((label.clone(), chip(label.clone(), Tone::Muted, cx)));
                }
            }
            let (view, slot_id, id) = (view.clone(), slot.to_string(), module.id.clone());
            radio_card(
                hashed_id("module", &(slot, &module.id)),
                selected.as_deref() == Some(module.id.as_str()),
                mono(module.id.clone(), cx),
                module.description.clone().map(Into::into),
                chips.into_iter().map(|(_, chip)| chip).collect(),
                move |cx| {
                    let (slot, id) = (slot_id.clone(), id.clone());
                    let _ = view.update(cx, |this, cx| {
                        this.update_draft(cx, |draft| draft.set_module(&slot, &id))
                    });
                },
                cx,
            )
        });
        let mut parts = vec![
            section(
                "Реализация",
                None,
                None,
                v_flex().gap_2().children(cards),
                cx,
            )
            .into_any_element(),
        ];
        if let Some(module) = selected {
            parts.push(self.params_section(slot, &module, cx));
        }
        parts
    }

    pub(super) fn context_providers_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(snapshot) = self.snapshot() else {
            return Vec::new();
        };
        let theme = cx.theme();
        let rows: Vec<AnyElement> = exports(snapshot, CONTEXT_PROVIDER_SLOT)
            .map(|(plugin, export)| {
                let key = format!("params:{CONTEXT_PROVIDER_SLOT}:{}", export.id);
                let open = self.is_open(&key);
                let header = h_flex().px_4().py_3().gap_3().items_start().child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(mono(export.id.clone(), cx).text_sm())
                        .children(export.description.clone().map(|text| {
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(text)
                        }))
                        .child(
                            h_flex()
                                .gap_1()
                                .child(chip(plugin.id.clone(), Tone::Muted, cx))
                                .child(if export.active {
                                    chip("подключён", Tone::Success, cx)
                                } else {
                                    chip("не подключён", Tone::Muted, cx)
                                }),
                        ),
                );
                v_flex()
                    .child(header)
                    .child(self.params_disclosure(
                        CONTEXT_PROVIDER_SLOT,
                        &export.id,
                        &key,
                        open,
                        cx,
                    ))
                    .into_any_element()
            })
            .collect();
        let providers = if rows.is_empty() {
            card(
                [note("Профиль не подключает поставщиков контекста.", cx).into_any_element()],
                cx,
            )
        } else {
            card(rows, cx)
        };
        let view = cx.weak_entity();
        let open_context = kit::button("open-context-slot")
            .label("Открыть Context")
            .ghost()
            .xsmall()
            .on_click(move |_, _, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.select_page(PageId::Slot("context".to_string()), cx)
                });
            });
        vec![
            section(
                "Поставщики",
                Some(
                    "Какие поставщики читаются автоматически, задают параметры модуля Context."
                        .into(),
                ),
                Some(open_context.into_any_element()),
                providers,
                cx,
            )
            .into_any_element(),
        ]
    }

    /// Collapsible parameters inside a card row.
    pub(super) fn params_disclosure(
        &self,
        slot: &str,
        module: &str,
        key: &str,
        open: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let changed =
            self.draft.module_config(slot, module) != self.baseline.module_config(slot, module);
        let view = cx.weak_entity();
        let toggle_key = key.to_string();
        v_flex()
            .child(
                super::widgets::disclosure(
                    hashed_id("params-open", &key),
                    open,
                    "Параметры",
                    changed,
                    move |cx| {
                        let _ = view.update(cx, |this, cx| this.toggle_open(&toggle_key, cx));
                    },
                    cx,
                )
                .border_t_1()
                .border_color(cx.theme().border),
            )
            .children(self.reveal(key, cx, || self.params_nested(slot, module, cx)))
            .into_any_element()
    }
}
