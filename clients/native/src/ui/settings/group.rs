//! Parameter rows of one object level, and an object field shown as a
//! foldable group of such rows.

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::domain::ConfigField;
use serde_json::Value;

use super::SettingsView;
use super::widgets::{changed_dot, described, disclosure, hashed_id, note, reset_mark};
use crate::settings::FieldPath;
use crate::ui::prefs;

impl SettingsView {
    /// Rows of `fields` living in `object`: ordinary fields, advanced ones
    /// folded, then a note on keys the schema does not describe.
    pub(super) fn field_rows(
        &self,
        fields: &[ConfigField],
        path_of: impl Fn(&str) -> FieldPath,
        object: Option<&Value>,
        scope: &str,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let field_row = |field: &ConfigField| self.render_field(&path_of(&field.key), field, cx);
        let mut rows: Vec<AnyElement> = fields
            .iter()
            .filter(|field| !field.advanced)
            .map(field_row)
            .collect();
        let advanced: Vec<_> = fields.iter().filter(|field| field.advanced).collect();
        if !advanced.is_empty() {
            let key = format!("advanced:{scope}");
            let open = self.is_open(&key);
            let changed = advanced.iter().any(|field| {
                let path = path_of(&field.key);
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
        let extra: Vec<&str> = object
            .and_then(Value::as_object)
            .map(|object| {
                object
                    .keys()
                    .filter(|key| !fields.iter().any(|field| &field.key == *key))
                    .map(String::as_str)
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
        rows
    }

    /// Object field: a header that folds its nested fields and resets them
    /// all at once.
    pub(super) fn render_group(
        &self,
        path: &FieldPath,
        field: &ConfigField,
        fields: &[ConfigField],
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let scope = format!("{}:{}:{}", path.slot, path.module, path.dotted());
        let key = format!("group:{scope}");
        let open = self.is_open(&key);
        let value = self.draft.value(path);
        let changed = value != self.baseline.value(path);
        let reset = value.is_some().then(|| {
            let (view, path) = (cx.weak_entity(), path.clone());
            reset_mark(
                hashed_id("group-reset", &path),
                "Своя группа · сбросить",
                move |_, cx| {
                    let _ = view.update(cx, |this, cx| this.reset_field(&path, cx));
                },
                cx,
            )
        });
        let view = cx.weak_entity();
        let toggle_key = key.clone();
        let header = h_flex()
            .id(hashed_id("group", path))
            .px_4()
            .py_2p5()
            .gap_2()
            .items_center()
            .text_sm()
            .cursor_pointer()
            .hover(|row| row.bg(theme.list_hover))
            .child(crate::ui::motion::chevron(
                ElementId::Name(format!("{key}-chevron").into()),
                open,
                theme.muted_foreground,
            ))
            .child(described(
                hashed_id("group-title", path),
                field.title.clone().into(),
                Some(field.description.clone().into()),
                Some(path.dotted().into()),
                cx,
            ))
            .children(reset)
            .when(changed, |title| title.child(changed_dot(cx)))
            .on_click(move |_, _, cx| {
                let _ = view.update(cx, |this, cx| this.toggle_open(&toggle_key, cx));
            });
        let content = self.reveal(&key, cx, || {
            let rows = self.field_rows(fields, |key| path.child(key), value, &scope, cx);
            div()
                .ml(prefs::scaled(24.))
                .mb_2()
                .border_l_2()
                .border_color(theme.border)
                .child(
                    v_flex().children(rows.into_iter().enumerate().map(|(ix, row)| {
                        div()
                            .when(ix > 0, |cell| cell.border_t_1().border_color(theme.border))
                            .child(row)
                    })),
                )
                .into_any_element()
        });
        v_flex().child(header).children(content).into_any_element()
    }
}
