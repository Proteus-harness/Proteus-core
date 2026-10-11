//! Parameters of one module: a form generated from its `config_schema`
//! or the raw JSON object.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::SettingsView;
use super::editors::module_key;
use super::widgets::{card, disclosure, hashed_id, note, section};
use crate::settings::FieldPath;
use crate::settings::catalog::module_info;

impl SettingsView {
    /// Parameters as a titled section of a page.
    pub(super) fn params_section(
        &self,
        slot: &str,
        module: &str,
        cx: &Context<Self>,
    ) -> AnyElement {
        section(
            "Параметры",
            None,
            self.mode_switch(slot, module, cx),
            self.params_body(slot, module, cx),
            cx,
        )
        .into_any_element()
    }

    /// Parameters nested in a card row (hooks, plugin exports): a mode
    /// switch on top and the editor below.
    pub(super) fn params_nested(&self, slot: &str, module: &str, cx: &Context<Self>) -> AnyElement {
        v_flex()
            .px_4()
            .pb_3()
            .gap_2()
            .child(
                h_flex()
                    .justify_end()
                    .children(self.mode_switch(slot, module, cx)),
            )
            .child(self.params_body(slot, module, cx))
            .into_any_element()
    }

    fn mode_switch(&self, slot: &str, module: &str, cx: &Context<Self>) -> Option<AnyElement> {
        let snapshot = self.snapshot()?;
        module_info(snapshot, slot, module)?.schema?;
        let json = self.is_json(slot, module);
        let (slot, module) = (slot.to_string(), module.to_string());
        Some(
            Button::new(hashed_id("params-mode", &(&slot, &module)))
                .label(if json { "Форма" } else { "JSON" })
                .ghost()
                .xsmall()
                .disabled(json && self.json_error(&slot, &module))
                .tooltip(if json {
                    "Вернуться к форме"
                } else {
                    "Редактировать параметры как JSON-объект"
                })
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_json(&slot, &module, cx)))
                .into_any_element(),
        )
    }

    fn params_body(&self, slot: &str, module: &str, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let Some(snapshot) = self.snapshot() else {
            return div().into_any_element();
        };
        let schema = module_info(snapshot, slot, module).and_then(|info| info.schema);
        if self.is_json(slot, module) {
            let field = self.editors.json.get(&module_key(slot, module));
            let hint = schema
                .is_none()
                .then_some("Описание параметров недоступно. Настройки можно изменить в JSON.");
            return v_flex()
                .gap_1p5()
                .children(field.map(|field| Textarea::new(&field.input).small().w_full()))
                .children(
                    field
                        .and_then(|field| field.error.clone())
                        .map(|error| div().text_xs().text_color(theme.danger).child(error)),
                )
                .children(hint.map(|hint| {
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(hint)
                }))
                .into_any_element();
        }
        let schema = schema.expect("form mode needs a schema");
        if schema.fields.is_empty() {
            return card(
                [note("У этого модуля нет настраиваемых параметров.", cx).into_any_element()],
                cx,
            )
            .into_any_element();
        }
        let field_row = |field: &proteus_contracts::domain::ConfigField| {
            self.render_field(&FieldPath::new(slot, module, &field.key), field, cx)
        };
        let mut rows: Vec<AnyElement> = schema
            .fields
            .iter()
            .filter(|field| !field.advanced)
            .map(field_row)
            .collect();
        let advanced: Vec<_> = schema
            .fields
            .iter()
            .filter(|field| field.advanced)
            .collect();
        if !advanced.is_empty() {
            let key = format!("advanced:{slot}:{module}");
            let open = self.is_open(&key);
            let changed = advanced.iter().any(|field| {
                let path = FieldPath::new(slot, module, &field.key);
                self.draft.value(&path) != self.baseline.value(&path)
            });
            let view = cx.weak_entity();
            let toggle_key = key.clone();
            rows.push(
                disclosure(
                    hashed_id("params-advanced", &key),
                    open,
                    format!("Дополнительно · {}", advanced.len()),
                    changed,
                    move |cx| {
                        let _ = view.update(cx, |this, cx| this.toggle_open(&toggle_key, cx));
                    },
                    cx,
                )
                .into_any_element(),
            );
            let border = cx.theme().border;
            rows.extend(self.reveal(&key, cx, || {
                v_flex()
                    .children(advanced.into_iter().enumerate().map(|(ix, field)| {
                        div()
                            .when(ix > 0, |cell| cell.border_t_1().border_color(border))
                            .child(field_row(field))
                    }))
                    .into_any_element()
            }));
        }
        let known: Vec<&str> = schema
            .fields
            .iter()
            .map(|field| field.key.as_str())
            .collect();
        let extra: Vec<String> = self
            .draft
            .module_config(slot, module)
            .and_then(|value| value.as_object())
            .map(|object| {
                object
                    .keys()
                    .filter(|key| !known.contains(&key.as_str()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if !extra.is_empty() {
            rows.push(
                note(
                    format!(
                        "Дополнительные параметры: {}. Они сохраняются вместе с профилем; изменить их можно в JSON.",
                        extra.join(", ")
                    ),
                    cx,
                )
                .into_any_element(),
            );
        }
        card(rows, cx).into_any_element()
    }
}
