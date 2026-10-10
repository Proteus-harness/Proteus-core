//! Rows inside settings cards: layout, selects and non-field items.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::SettingsView;
use super::nav::changed_dot;
use crate::settings::{Choice, Draft, Item};
use crate::ui::controls::mode_label;
use crate::ui::format::clip;

const TOOL_DESCRIPTION: usize = 160;

pub(super) fn hashed_id(name: &'static str, key: &impl Hash) -> ElementId {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    ElementId::NamedInteger(name.into(), hasher.finish())
}

/// Title and description on the left, the control on the right; wide
/// controls go under the text instead.
pub(super) fn row(
    title: impl IntoElement,
    description: Option<SharedString>,
    control: impl IntoElement,
    changed: bool,
    vertical: bool,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let text = v_flex()
        .gap_0p5()
        .min_w_0()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .text_sm()
                .child(title)
                .when(changed, |title| title.child(changed_dot(cx))),
        )
        .children(description.map(|text| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(text)
        }));
    if vertical {
        v_flex()
            .px_4()
            .py_3()
            .gap_2()
            .child(text)
            .child(div().w_full().child(control))
    } else {
        h_flex()
            .px_4()
            .py_3()
            .gap_6()
            .items_center()
            .child(text.flex_1())
            .child(div().flex_none().child(control))
    }
}

pub(super) struct MenuOption<T> {
    pub label: SharedString,
    pub value: T,
    pub checked: bool,
}

/// Outline button opening a checked list; `select` receives the picked value.
pub(super) fn select_menu<T: Clone + 'static>(
    id: ElementId,
    label: impl Into<SharedString>,
    options: Vec<MenuOption<T>>,
    select: impl Fn(T, &mut App) + 'static,
) -> AnyElement {
    let select = Rc::new(select);
    let options = Rc::new(options);
    Button::new(id)
        .label(label)
        .outline()
        .small()
        .dropdown_caret(true)
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu: PopupMenu, _, _| {
            let menu = menu.min_w(px(200.)).max_h(px(360.)).scrollable(true);
            options.iter().fold(menu, |menu, option| {
                let (select, value) = (select.clone(), option.value.clone());
                menu.item(
                    PopupMenuItem::new(option.label.clone())
                        .checked(option.checked)
                        .on_click(move |_, _, cx| select(value.clone(), cx)),
                )
            })
        })
        .into_any_element()
}

impl SettingsView {
    pub(super) fn render_item(&self, item: &Item, cx: &Context<Self>) -> AnyElement {
        let changed = item.changed(&self.draft, &self.baseline);
        let view = cx.weak_entity();
        let edit = move |cx: &mut App, change: Box<dyn FnOnce(&mut Draft)>| {
            let _ = view.update(cx, |this, cx| this.update_draft(cx, change));
        };
        match item {
            Item::Provider(choices) => {
                let current = self.draft.provider.clone().unwrap_or_default();
                let control =
                    choice_select("settings-provider", choices, &current, move |value, cx| {
                        edit(cx, Box::new(move |draft| draft.provider = Some(value)))
                    });
                row(
                    "Провайдер",
                    Some("Модель, которой агент отправляет запросы.".into()),
                    control,
                    changed,
                    false,
                    cx,
                )
                .into_any_element()
            }
            Item::Mode(modes) => {
                let choices: Vec<Choice> = modes
                    .iter()
                    .map(|mode| Choice {
                        value: mode.clone(),
                        label: mode_label(mode).to_string(),
                    })
                    .collect();
                let control = choice_select(
                    "settings-mode",
                    &choices,
                    &self.draft.mode,
                    move |value, cx| edit(cx, Box::new(move |draft| draft.mode = value)),
                );
                row(
                    "Режим доступа",
                    Some("Режим по умолчанию для новых запусков этого профиля.".into()),
                    control,
                    changed,
                    false,
                    cx,
                )
                .into_any_element()
            }
            Item::Module {
                slot,
                options,
                description,
            } => {
                let current = self.draft.modules.get(slot).cloned().unwrap_or_default();
                let control = if options.len() > 1 {
                    let slot = slot.clone();
                    choice_select(
                        hashed_id("settings-module", &slot),
                        options,
                        &current,
                        move |value, cx| {
                            let slot = slot.clone();
                            edit(cx, Box::new(move |draft| draft.set_module(&slot, &value)))
                        },
                    )
                    .into_any_element()
                } else {
                    div()
                        .text_sm()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(cx.theme().muted_foreground)
                        .child(if current.is_empty() {
                            "—".into()
                        } else {
                            current
                        })
                        .into_any_element()
                };
                row(
                    "Модуль",
                    Some(
                        description
                            .clone()
                            .unwrap_or_else(|| format!("Реализация слота {slot}."))
                            .into(),
                    ),
                    control,
                    changed,
                    false,
                    cx,
                )
                .into_any_element()
            }
            Item::Hook { id, description } => {
                let enabled = self.draft.hooks.contains(id);
                let hook = id.clone();
                let control = Switch::new(hashed_id("settings-hook", id))
                    .checked(enabled)
                    .on_click(move |on, _, cx| {
                        let (hook, on) = (hook.clone(), *on);
                        edit(cx, Box::new(move |draft| draft.set_hook(&hook, on)))
                    });
                row(
                    id.clone(),
                    description.clone().map(Into::into),
                    control,
                    changed,
                    false,
                    cx,
                )
                .into_any_element()
            }
            Item::Tool {
                name,
                description,
                locked,
            } => {
                let enabled = self.draft.tools.contains(name);
                let tool = name.clone();
                let control = Switch::new(hashed_id("settings-tool", name))
                    .checked(enabled)
                    .disabled(*locked)
                    .when(*locked, |switch| {
                        switch.tooltip("Не переключается в этом профиле")
                    })
                    .on_click(move |on, _, cx| {
                        let (tool, on) = (tool.clone(), *on);
                        edit(cx, Box::new(move |draft| draft.set_tool(&tool, on)))
                    });
                let description = clip(description, TOOL_DESCRIPTION);
                row(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(name.clone()),
                    (!description.is_empty()).then(|| description.into()),
                    control,
                    changed,
                    false,
                    cx,
                )
                .into_any_element()
            }
            Item::Field { path, field } => self.render_field(path, field, cx),
            Item::Note(text) => div()
                .px_4()
                .py_3()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(text.clone())
                .into_any_element(),
        }
    }
}

fn choice_select(
    id: impl Into<ElementId>,
    choices: &[Choice],
    current: &str,
    select: impl Fn(String, &mut App) + 'static,
) -> AnyElement {
    let label = choices
        .iter()
        .find(|choice| choice.value == current)
        .map(|choice| choice.label.clone())
        .unwrap_or_else(|| {
            if current.is_empty() {
                "Не выбрано".to_string()
            } else {
                current.to_string()
            }
        });
    let options = choices
        .iter()
        .map(|choice| MenuOption {
            label: choice.label.clone().into(),
            value: choice.value.clone(),
            checked: choice.value == current,
        })
        .collect();
    select_menu(id.into(), label, options, select)
}
