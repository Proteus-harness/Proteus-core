//! Rows for module `config_schema` fields.

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::domain::{ConfigChoice, ConfigField};
use serde_json::Value;

use super::SettingsView;
use super::editors::TextInput;
use super::widgets::{MenuOption, Tone, changed_dot, chip, hashed_id, row, select_menu};
use crate::settings::{FieldKind, FieldPath, choice_title, default_hint, field_kind};

const LINE_INPUT_WIDTH: f32 = 260.;
const CHOICE_WIDTH: f32 = 220.;

/// Everything one field control needs inside its event handlers.
struct FieldSpec {
    path: FieldPath,
    field: ConfigField,
    value: Option<Value>,
    view: WeakEntity<SettingsView>,
}

impl FieldSpec {
    fn set(&self, value: Option<Value>, cx: &mut App) {
        let path = self.path.clone();
        let _ = self.view.update(cx, |this, cx| {
            this.update_draft(cx, |draft| draft.set_value(&path, value))
        });
    }

    fn reset(&self, cx: &mut App) {
        let path = self.path.clone();
        let _ = self.view.update(cx, |this, cx| this.reset_field(&path, cx));
    }

    fn effective(&self) -> Option<&Value> {
        self.value.as_ref().or(self.field.default.as_ref())
    }

    fn id(&self, name: &'static str) -> ElementId {
        hashed_id(name, &self.path)
    }
}

impl SettingsView {
    pub(super) fn render_field(
        &self,
        path: &FieldPath,
        field: &ConfigField,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let kind = field_kind(&field.value);
        let text = self.editors.fields.get(path);
        let spec = Rc::new(FieldSpec {
            path: path.clone(),
            field: field.clone(),
            value: self.draft.value(path).cloned(),
            view: cx.weak_entity(),
        });
        let vertical = match kind {
            FieldKind::MultiChoice(_) => true,
            FieldKind::Text(format) => format.is_multiline(),
            _ => false,
        };
        let control = match kind {
            FieldKind::Switch => {
                let checked = spec.effective().and_then(Value::as_bool).unwrap_or(false);
                let handler = spec.clone();
                Switch::new(spec.id("switch"))
                    .checked(checked)
                    .on_click(move |checked, _, cx| handler.set(Some(Value::Bool(*checked)), cx))
                    .into_any_element()
            }
            FieldKind::Choice(options) => choice_menu(&spec, options),
            FieldKind::MultiChoice(options) => multi_choice(&spec, options),
            FieldKind::Text(_) => match text.map(|text| &text.input) {
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
        let unit = field
            .unit
            .clone()
            .filter(|unit| !unit.is_empty() && !vertical)
            .map(|unit| {
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(unit)
            });
        let handler = spec.clone();
        let reset = Button::new(spec.id("reset"))
            .icon(IconName::Undo2)
            .ghost()
            .xsmall()
            .disabled(spec.value.is_none())
            .tooltip(format!("Сбросить «{}»", field.title))
            .on_click(move |_, _, cx| handler.reset(cx));
        let error = text
            .and_then(|text| text.error.clone())
            .map(|error| div().text_xs().text_color(theme.danger).child(error));
        let control = v_flex()
            .gap_1()
            .when(vertical, |column| column.w_full())
            .when(!vertical, |column| column.items_end())
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .when(vertical, |line| line.w_full().items_start())
                    .child(
                        div()
                            .when(vertical, |cell| cell.flex_1().min_w_0())
                            .child(control),
                    )
                    .children(unit)
                    .child(reset),
            )
            .children(error);

        let changed = self.draft.value(path) != self.baseline.value(path);
        let origin = if spec.value.is_some() {
            Some(chip("Своё значение", Tone::Accent, cx))
        } else if field.required && field.default.is_none() {
            Some(chip("Нужно задать", Tone::Warning, cx))
        } else {
            None
        };
        let title = h_flex()
            .id(spec.id("title"))
            .gap_2()
            .items_center()
            .child(field.title.clone())
            .children(origin)
            .when(changed, |title| title.child(changed_dot(cx)))
            .tooltip({
                let key = field.key.clone();
                move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(key.clone()).build(window, cx)
                }
            });
        let description = field.description.trim();
        row(
            title,
            (!description.is_empty()).then(|| description.to_string().into()),
            control,
            vertical,
            cx,
        )
        .into_any_element()
    }
}

fn choice_menu(spec: &Rc<FieldSpec>, options: &[ConfigChoice]) -> AnyElement {
    let label = match &spec.value {
        Some(value) => choice_title(options, value)
            .map(str::to_string)
            .unwrap_or_else(|| format!("Неизвестное значение: {value}")),
        None => match default_hint(&spec.field) {
            Some(hint) => format!("По умолчанию ({hint})"),
            None => "Не задано".to_string(),
        },
    };
    let mut items = Vec::with_capacity(options.len() + 1);
    if !spec.field.required {
        items.push(MenuOption {
            label: "По умолчанию".into(),
            value: None,
            checked: spec.value.is_none(),
        });
    }
    items.extend(options.iter().map(|choice| MenuOption {
        label: choice.title.clone().into(),
        value: Some(choice.value.clone()),
        checked: spec.value.as_ref() == Some(&choice.value),
    }));
    let handler = spec.clone();
    select_menu(spec.id("choice"), label, items, move |value, cx| {
        handler.set(value, cx)
    })
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
        div().w(px(CHOICE_WIDTH)).child(
            Checkbox::new(hashed_id("multi", &(&spec.path, ix)))
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
                }),
        )
    });
    h_flex()
        .flex_wrap()
        .gap_x_4()
        .gap_y_2()
        .children(boxes)
        .into_any_element()
}
