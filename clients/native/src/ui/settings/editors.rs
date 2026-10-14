//! Text controls of the settings screen. Inputs keep their own text, so they
//! are created for the parameters visible on the current page and rebuilt
//! whenever the draft changes from outside them.

use std::collections::HashMap;

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::*;
use serde_json::Value;

use super::SettingsView;
use crate::settings::catalog::{
    self, CONTEXT_PROVIDER_SLOT, CONTEXT_SLOT, HOOK_SLOT, MODEL_SLOT, PageId, TOOL_SLOT,
    module_info,
};
use crate::settings::{
    FieldKind, FieldPath, TextFormat, default_hint, field_kind, format_value, parse_text,
};

#[derive(Clone)]
pub(super) enum TextInput {
    Line(Entity<InputState>),
    Area(Entity<TextareaState>),
}

pub(super) struct TextField {
    pub input: TextInput,
    pub error: Option<SharedString>,
    _subscription: Subscription,
}

pub(super) struct JsonField {
    pub input: Entity<TextareaState>,
    pub error: Option<SharedString>,
    _subscription: Subscription,
}

pub(super) type ModuleKey = (String, String);

#[derive(Default)]
pub(super) struct Editors {
    pub fields: HashMap<FieldPath, TextField>,
    pub json: HashMap<ModuleKey, JsonField>,
}

impl Editors {
    pub fn has_errors(&self) -> bool {
        self.fields.values().any(|field| field.error.is_some())
            || self.json.values().any(|field| field.error.is_some())
    }

    fn forget_form(&mut self, slot: &str, module: &str) {
        self.fields
            .retain(|path, _| path.slot != slot || path.module != module);
    }
}

pub(super) fn module_key(slot: &str, module: &str) -> ModuleKey {
    (slot.to_string(), module.to_string())
}

impl SettingsView {
    /// Modules whose parameter editors the current page can show.
    fn visible_modules(&self) -> Vec<ModuleKey> {
        let Some(snapshot) = self.snapshot() else {
            return Vec::new();
        };
        match &self.page {
            PageId::Model => catalog::model_module(snapshot, &self.draft)
                .map(|module| vec![module_key(MODEL_SLOT, &module)])
                .unwrap_or_default(),
            PageId::Slot(slot) => catalog::selected_module(snapshot, &self.draft, slot)
                .map(|module| module_key(slot, module))
                .into_iter()
                .chain(
                    catalog::exports(snapshot, CONTEXT_PROVIDER_SLOT)
                        .filter(|_| slot == CONTEXT_SLOT)
                        .map(|(_, export)| module_key(&export.slot, &export.id)),
                )
                .collect(),
            PageId::Hooks => self
                .draft
                .hooks
                .iter()
                .map(|hook| module_key(HOOK_SLOT, hook))
                .collect(),
            PageId::Modules => snapshot
                .plugins
                .iter()
                .flat_map(|plugin| &plugin.exports)
                .filter(|export| [TOOL_SLOT, HOOK_SLOT].contains(&export.slot.as_str()))
                .map(|export| module_key(&export.slot, &export.id))
                .collect(),
            _ => Vec::new(),
        }
    }

    pub(super) fn is_json(&self, slot: &str, module: &str) -> bool {
        self.json.contains(&module_key(slot, module))
            || self
                .snapshot()
                .and_then(|snapshot| module_info(snapshot, slot, module))
                .is_none_or(|info| info.schema.is_none())
    }

    pub(super) fn prepare_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (slot, module) in self.visible_modules() {
            if self.is_json(&slot, &module) {
                self.ensure_json(&slot, &module, window, cx);
                continue;
            }
            let fields: Vec<_> = self
                .snapshot()
                .and_then(|snapshot| module_info(snapshot, &slot, &module))
                .and_then(|info| info.schema)
                .map(|schema| schema.fields.clone())
                .unwrap_or_default();
            for field in fields {
                if let FieldKind::Text(format) = field_kind(&field.value) {
                    let path = FieldPath::new(&slot, &module, &field.key);
                    if !self.editors.fields.contains_key(&path) {
                        let placeholder = default_hint(&field)
                            .map(|hint| format!("По умолчанию: {hint}"))
                            .unwrap_or_else(|| "Не задано".to_string());
                        self.create_field(path, format, placeholder, window, cx);
                    }
                }
            }
        }
    }

    fn create_field(
        &mut self,
        path: FieldPath,
        format: TextFormat,
        placeholder: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self
            .draft
            .value(&path)
            .map(|value| format_value(format, value))
            .unwrap_or_default();
        let key = path.clone();
        let (input, subscription) = if format.is_multiline() {
            let state = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(2, 12)
                    .placeholder(placeholder)
                    .default_value(text)
            });
            let subscription = cx.subscribe_in(&state, window, move |this, state, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = state.read(cx).value();
                    this.text_changed(&key, format, &text, cx);
                }
            });
            (TextInput::Area(state), subscription)
        } else {
            let state = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .masked(format == TextFormat::Secret)
                    .default_value(text)
            });
            let subscription = cx.subscribe_in(&state, window, move |this, state, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = state.read(cx).value();
                    this.text_changed(&key, format, &text, cx);
                }
            });
            (TextInput::Line(state), subscription)
        };
        self.editors.fields.insert(
            path,
            TextField {
                input,
                error: None,
                _subscription: subscription,
            },
        );
    }

    fn text_changed(
        &mut self,
        path: &FieldPath,
        format: TextFormat,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let parsed = parse_text(format, text);
        if let Some(field) = self.editors.fields.get_mut(path) {
            field.error = parsed.as_ref().err().map(|error| error.clone().into());
        }
        match parsed {
            Ok(value) => self.update_draft(cx, |draft| draft.set_value(path, value)),
            Err(_) => cx.notify(),
        }
    }

    fn ensure_json(
        &mut self,
        slot: &str,
        module: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = module_key(slot, module);
        if self.editors.json.contains_key(&key) {
            return;
        }
        let value = self
            .draft
            .module_config(slot, module)
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));
        let text = serde_json::to_string_pretty(&value).unwrap_or_default();
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 18)
                .placeholder("{}")
                .default_value(text)
        });
        let target = key.clone();
        let subscription = cx.subscribe_in(&input, window, move |this, state, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                let text = state.read(cx).value();
                this.json_changed(&target, &text, cx);
            }
        });
        self.editors.json.insert(
            key,
            JsonField {
                input,
                error: None,
                _subscription: subscription,
            },
        );
    }

    fn json_changed(&mut self, key: &ModuleKey, text: &str, cx: &mut Context<Self>) {
        let parsed =
            match serde_json::from_str::<Value>(if text.trim().is_empty() { "{}" } else { text }) {
                Ok(Value::Object(object)) => Ok(object),
                Ok(_) => Err("Параметры должны быть JSON-объектом".to_string()),
                Err(error) => Err(format!("Неверный JSON: {error}")),
            };
        if let Some(field) = self.editors.json.get_mut(key) {
            field.error = parsed.as_ref().err().map(|error| error.clone().into());
        }
        match parsed {
            Ok(object) => {
                let (slot, module) = key;
                self.update_draft(cx, |draft| draft.set_module_config(slot, module, object));
            }
            Err(_) => cx.notify(),
        }
    }

    /// Switches one module between the form and raw JSON; each side is
    /// rebuilt from the draft.
    pub(super) fn toggle_json(&mut self, slot: &str, module: &str, cx: &mut Context<Self>) {
        let key = module_key(slot, module);
        if self.json.remove(&key) {
            self.editors.json.remove(&key);
        } else {
            self.editors.forget_form(slot, module);
            self.json.insert(key);
        }
        cx.notify();
    }

    pub(super) fn json_error(&self, slot: &str, module: &str) -> bool {
        self.editors
            .json
            .get(&module_key(slot, module))
            .is_some_and(|field| field.error.is_some())
    }

    pub(super) fn reset_field(&mut self, path: &FieldPath, cx: &mut Context<Self>) {
        self.editors.fields.remove(path);
        self.update_draft(cx, |draft| draft.set_value(path, None));
    }
}
