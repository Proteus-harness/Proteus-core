//! Rows for module `config_schema` fields.

use std::rc::Rc;

use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::domain::{ConfigChoice, ConfigField};
use serde_json::Value;

use super::SettingsView;
use super::editors::TextInput;
use super::widgets::{
    Fit, MenuOption, SHORT_INPUT_WIDTH, Tone, changed_dot, chip, described, field_row, hashed_id,
    reset_mark, select_menu,
};
use crate::settings::{FieldKind, FieldPath, TextFormat, choice_title, field_kind};
use crate::ui::kit;
use crate::ui::prefs;

const CHOICE_WIDTH: f32 = 220.;

/// Everything one field control needs inside its event handlers.
struct FieldSpec {
    path: FieldPath,
    field: ConfigField,
    value: Option<Value>,
    view: WeakEntity<SettingsView>,
}

impl FieldSpec {
    /// Stores `value`; picking the default again drops the key, so the
    /// profile and the draft stay as if it was never touched.
    fn set(&self, value: Option<Value>, cx: &mut App) {
        let value = value.filter(|value| Some(value) != self.field.default.as_ref());
        let path = self.path.clone();
        let _ = self.view.update(cx, |this, cx| {
            this.update_draft(cx, |draft| draft.set_value(&path, value))
        });
    }

    /// A value of its own that differs from the default.
    fn own(&self) -> bool {
        self.value
            .as_ref()
            .is_some_and(|value| Some(value) != self.field.default.as_ref())
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
        let kind = field_kind(field);
        let text = self.editors.fields.get(path);
        let spec = Rc::new(FieldSpec {
            path: path.clone(),
            field: field.clone(),
            value: self.draft.value(path).cloned(),
            view: cx.weak_entity(),
        });
        let short = matches!(
            kind,
            FieldKind::Text(TextFormat::Integer { .. } | TextFormat::Number | TextFormat::Value)
        );
        let fit = match kind {
            FieldKind::MultiChoice(_) => Fit::Below,
            FieldKind::Text(format) if format.is_multiline() => Fit::Below,
            FieldKind::Text(_) if !short => Fit::Shrink,
            _ => Fit::Fixed,
        };
        let vertical = matches!(fit, Fit::Below);
        let stretch = !matches!(fit, Fit::Fixed);
        let control = match kind {
            FieldKind::Switch => {
                let checked = spec.effective().and_then(Value::as_bool).unwrap_or(false);
                let handler = spec.clone();
                kit::switch(spec.id("switch"))
                    .checked(checked)
                    .on_click(move |checked, _, cx| handler.set(Some(Value::Bool(*checked)), cx))
                    .into_any_element()
            }
            FieldKind::Choice(options) => choice_menu(&spec, options),
            FieldKind::MultiChoice(options) => multi_choice(&spec, options),
            FieldKind::Group(fields) => return self.render_group(path, field, fields, cx),
            FieldKind::Text(_) => match text.map(|text| &text.input) {
                Some(TextInput::Line(state)) => Input::new(state)
                    .small()
                    .map(|input| {
                        if short {
                            input.w(prefs::scaled(SHORT_INPUT_WIDTH))
                        } else {
                            input.w_full()
                        }
                    })
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
                    .flex_none()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(unit)
            });
        let error = text
            .and_then(|text| text.error.clone())
            .map(|error| div().text_xs().text_color(theme.danger).child(error));
        let control = v_flex()
            .gap_1()
            .when(stretch, |column| column.w_full())
            .when(!stretch, |column| column.items_end())
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .when(vertical, |line| line.items_start())
                    .when(stretch, |line| line.w_full())
                    .child(
                        div()
                            .when(stretch, |cell| cell.flex_1().min_w_0())
                            .child(control),
                    )
                    .children(unit),
            )
            .children(error);

        let changed = self.draft.value(path) != self.baseline.value(path);
        let reset = spec.own().then(|| {
            let handler = spec.clone();
            reset_mark(
                spec.id("reset"),
                "Своё значение · сбросить",
                move |_, cx| handler.reset(cx),
                cx,
            )
        });
        let missing = (spec.value.is_none() && field.required && field.default.is_none())
            .then(|| chip("Нужно задать", Tone::Warning, cx));
        let title = h_flex()
            .min_w_0()
            .gap_1()
            .items_center()
            .child(described(
                spec.id("title"),
                field.title.clone().into(),
                Some(field.description.clone().into()),
                Some(path.dotted().into()),
                cx,
            ))
            .children(reset)
            .children(missing)
            .when(changed, |title| title.child(changed_dot(cx)));
        self.spotted(
            &super::spot::field(path),
            field_row(title, control, fit),
            cx,
        )
    }
}

/// Shows the value in effect; with a module default there is nothing like
/// "default" to pick: choosing the default option drops the key.
fn choice_menu(spec: &Rc<FieldSpec>, options: &[ConfigChoice]) -> AnyElement {
    let label = match spec.effective() {
        Some(value) => choice_title(options, value)
            .map(str::to_string)
            .unwrap_or_else(|| format!("Неизвестное значение: {value}")),
        None => "Не задано".to_string(),
    };
    let mut items = Vec::with_capacity(options.len() + 1);
    if !spec.field.required && spec.field.default.is_none() {
        items.push(MenuOption {
            label: "Не задано".into(),
            value: None,
            checked: spec.value.is_none(),
        });
    }
    items.extend(options.iter().map(|choice| MenuOption {
        label: choice.title.clone().into(),
        value: Some(choice.value.clone()),
        checked: spec.effective() == Some(&choice.value),
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
        div().w(prefs::scaled(CHOICE_WIDTH)).child(
            kit::checkbox(hashed_id("multi", &(&spec.path, ix)))
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
