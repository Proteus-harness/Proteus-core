//! Parameters of one module: a form generated from its `config_schema`
//! or the raw JSON object.

use std::path::Path;

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::Textarea;
use gpui_kit::component::*;
use gpui_kit::*;

use super::SettingsView;
use super::editors::module_key;
use super::widgets::{card, hashed_id, note, section};
use crate::settings::FieldPath;
use crate::settings::catalog::module_info;
use crate::ui::kit;
use crate::ui::kit::Hint as _;

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

    /// Opening the profile file and switching between the form and JSON.
    fn mode_switch(&self, slot: &str, module: &str, cx: &Context<Self>) -> Option<AnyElement> {
        let snapshot = self.snapshot()?;
        let file = snapshot.target_path.clone().map(|path| {
            kit::button(hashed_id("params-file", &(slot, module)))
                .label("Файл профиля")
                .ghost()
                .xsmall()
                .hint(
                    &format!("Открыть [module_config.{slot}.{module}] в {path}"),
                    cx,
                )
                .on_click(move |_, _, cx| cx.open_with_system(Path::new(&path)))
        });
        let mode = module_info(snapshot, slot, module)?.schema.map(|_| {
            let json = self.is_json(slot, module);
            let (slot, module) = (slot.to_string(), module.to_string());
            kit::button(hashed_id("params-mode", &(&slot, &module)))
                .label(if json { "Форма" } else { "JSON" })
                .ghost()
                .xsmall()
                .disabled(json && self.json_error(&slot, &module))
                .hint(
                    if json {
                        "Вернуться к форме"
                    } else {
                        "Править как JSON"
                    },
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_json(&slot, &module, cx)))
        });
        (file.is_some() || mode.is_some()).then(|| {
            h_flex()
                .gap_1()
                .children(file)
                .children(mode)
                .into_any_element()
        })
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
                .children(
                    field
                        .map(|field| Self::text_keys(Textarea::new(&field.input).small().w_full())),
                )
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
        let rows = self.field_rows(
            &schema.fields,
            |key| FieldPath::new(slot, module, key),
            self.draft.module_config(slot, module),
            &format!("{slot}:{module}"),
            cx,
        );
        card(rows, cx).into_any_element()
    }
}
