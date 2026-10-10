use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::setting::{SettingField, SettingItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::domain::{ConfigChoice, ConfigField};
use serde_json::Value;

use super::{SettingsView, TextInput};
use crate::settings::{FieldKind, FieldPath, choice_title, default_hint, field_kind};

const LINE_INPUT_WIDTH: f32 = 280.;

/// Everything one field control needs; captured by the settings framework's
/// render closure, which runs every frame.
struct FieldSpec {
    path: FieldPath,
    field: ConfigField,
    value: Option<Value>,
    input: Option<TextInput>,
    error: Option<SharedString>,
    view: WeakEntity<SettingsView>,
}

impl FieldSpec {
    fn set(&self, value: Option<Value>, cx: &mut App) {
        let path = self.path.clone();
        let _ = self
            .view
            .update(cx, |this, cx| this.set_field(&path, value, cx));
    }

    fn effective(&self) -> Option<&Value> {
        self.value.as_ref().or(self.field.default.as_ref())
    }

    fn id(&self, name: &'static str) -> ElementId {
        let mut hasher = DefaultHasher::new();
        self.path.hash(&mut hasher);
        ElementId::NamedInteger(name.into(), hasher.finish())
    }
}

impl SettingsView {
    pub(super) fn field_item(
        &self,
        path: &FieldPath,
        field: &ConfigField,
        view: &WeakEntity<Self>,
    ) -> SettingItem {
        let kind = field_kind(&field.value);
        let text = self.inputs.get(path);
        let spec = Rc::new(FieldSpec {
            path: path.clone(),
            field: field.clone(),
            value: self.draft.value(path).cloned(),
            input: text.map(|text| text.input.clone()),
            error: text.and_then(|text| text.error.clone()),
            view: view.clone(),
        });
        let vertical = match kind {
            FieldKind::MultiChoice(_) => true,
            FieldKind::Text(format) => format.is_multiline(),
            _ => false,
        };
        let item = SettingItem::new(
            field.title.clone(),
            SettingField::render(move |_, _, cx| render_control(&spec, vertical, cx)),
        )
        .description(SharedString::from(describe(field)))
        .keywords([field.key.clone()]);
        if vertical {
            item.layout(Axis::Vertical)
        } else {
            item
        }
    }
}

fn describe(field: &ConfigField) -> String {
    let mut text = field.description.trim().to_string();
    if let Some(unit) = field.unit.as_deref().filter(|unit| !unit.is_empty()) {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&format!("({unit})"));
    }
    if text.is_empty() {
        text = field.key.clone();
    }
    text
}

fn render_control(spec: &Rc<FieldSpec>, vertical: bool, cx: &mut App) -> AnyElement {
    let theme = cx.theme().clone();
    let control = match field_kind(&spec.field.value) {
        FieldKind::Switch => {
            let checked = spec.effective().and_then(Value::as_bool).unwrap_or(false);
            let handler = spec.clone();
            Switch::new(spec.id("switch"))
                .checked(checked)
                .on_click(move |checked, _, cx| handler.set(Some(Value::Bool(*checked)), cx))
                .into_any_element()
        }
        FieldKind::Choice(options) => choice_menu(spec, options),
        FieldKind::MultiChoice(options) => multi_choice(spec, options),
        FieldKind::Text(_) => match &spec.input {
            Some(TextInput::Line(state)) => Input::new(state)
                .small()
                .w(px(LINE_INPUT_WIDTH))
                .into_any_element(),
            Some(TextInput::Area(state)) => {
                Textarea::new(state).small().w_full().into_any_element()
            }
            None => div().into_any_element(),
        },
    };
    let reset = spec.value.is_some().then(|| {
        let handler = spec.clone();
        Button::new(spec.id("reset"))
            .icon(IconName::Undo2)
            .ghost()
            .xsmall()
            .tooltip("Сбросить к значению модуля")
            .on_click(move |_, _, cx| {
                let path = handler.path.clone();
                let _ = handler
                    .view
                    .update(cx, |this, cx| this.reset_field(&path, cx));
            })
    });
    let error = spec
        .error
        .clone()
        .map(|error| div().text_xs().text_color(theme.danger).child(error));

    v_flex()
        .gap_1()
        .when(vertical, |column| column.w_full())
        .when(!vertical, |column| column.items_end())
        .child(
            h_flex()
                .gap_1()
                .items_start()
                .when(vertical, |row| row.w_full())
                .child(
                    div()
                        .when(vertical, |cell| cell.flex_1().min_w_0())
                        .child(control),
                )
                .children(reset),
        )
        .children(error)
        .into_any_element()
}

fn choice_menu(spec: &Rc<FieldSpec>, options: &[ConfigChoice]) -> AnyElement {
    let label = match &spec.value {
        Some(value) => choice_title(options, value)
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string()),
        None => match default_hint(&spec.field) {
            Some(hint) => format!("По умолчанию ({hint})"),
            None => "По умолчанию".to_string(),
        },
    };
    let options = options.to_vec();
    let handler = spec.clone();
    Button::new(spec.id("choice"))
        .label(label)
        .outline()
        .small()
        .dropdown_caret(true)
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu: PopupMenu, _, _| {
            let mut menu = menu.max_h(px(360.)).scrollable(true);
            if !handler.field.required {
                let reset = handler.clone();
                menu = menu.item(
                    PopupMenuItem::new("По умолчанию")
                        .checked(handler.value.is_none())
                        .on_click(move |_, _, cx| reset.set(None, cx)),
                );
            }
            options.iter().fold(menu, |menu, choice| {
                let (select, value) = (handler.clone(), choice.value.clone());
                let checked = handler.value.as_ref() == Some(&choice.value);
                menu.item(
                    PopupMenuItem::new(choice.title.clone())
                        .checked(checked)
                        .on_click(move |_, _, cx| select.set(Some(value.clone()), cx)),
                )
            })
        })
        .into_any_element()
}

fn multi_choice(spec: &Rc<FieldSpec>, options: &[ConfigChoice]) -> AnyElement {
    let selected: Vec<Value> = spec
        .effective()
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let boxes = options.iter().enumerate().map(|(ix, choice)| {
        let checked = selected.contains(&choice.value);
        let (handler, options, value) = (spec.clone(), options.to_vec(), choice.value.clone());
        let selected = selected.clone();
        let id = match spec.id("multi") {
            ElementId::NamedInteger(name, hash) => {
                ElementId::NamedInteger(name, hash.wrapping_add(ix as u64))
            }
            other => other,
        };
        Checkbox::new(id)
            .checked(checked)
            .label(choice.title.clone())
            .on_click(move |checked, _, cx| {
                let next: Vec<Value> = options
                    .iter()
                    .map(|option| &option.value)
                    .filter(|option| {
                        if **option == value {
                            *checked
                        } else {
                            selected.contains(option)
                        }
                    })
                    .cloned()
                    .collect();
                handler.set(Some(Value::Array(next)), cx);
            })
    });
    v_flex().gap_1p5().children(boxes).into_any_element()
}
