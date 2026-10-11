//! Selectors of the session's permission mode, model and reasoning effort.
//! They are shown as extension icons, so they are built from a config and a
//! chat handle rather than inside the chat view.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::app_protocol::config::ConfigSummary;
use proteus_contracts::domain::PermissionMode;

use super::Chat;

const MODES: [(PermissionMode, &str); 3] = [
    (PermissionMode::Normal, "По правилам"),
    (PermissionMode::Auto, "Правки без вопросов"),
    (PermissionMode::Plan, "Планирование"),
];

/// Which session setting a selector changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum Control {
    Mode,
    Model,
    Effort,
}

/// Russian label of a persisted permission mode string.
pub(in crate::ui) fn mode_label(value: &str) -> &'static str {
    let mode = mode_from_value(value);
    MODES
        .iter()
        .find(|(candidate, _)| *candidate == mode)
        .map_or("По правилам", |(_, label)| label)
}

fn mode_from_value(value: &str) -> PermissionMode {
    match value.to_ascii_lowercase().as_str() {
        "plan" => PermissionMode::Plan,
        "auto" => PermissionMode::Auto,
        _ => PermissionMode::Normal,
    }
}

/// Effort override sent to the server: `None` keeps the profile default.
fn current_effort(config: &ConfigSummary) -> Option<String> {
    config
        .reasoning
        .effort
        .clone()
        .or_else(|| (!config.reasoning.enabled).then(|| "none".to_owned()))
}

fn effort_label(effort: Option<&str>) -> String {
    match effort {
        None => "рассуждения: профиль".to_owned(),
        Some("none") => "без рассуждений".to_owned(),
        Some(value) => format!("рассуждения: {value}"),
    }
}

/// Current reasoning effort of a session, as its selector shows it.
pub(in crate::ui) fn effort_of(config: &ConfigSummary) -> String {
    effort_label(current_effort(config).as_deref())
}

fn model_label(config: &ConfigSummary) -> String {
    config
        .model
        .as_ref()
        .map(|model| {
            if model.label.is_empty() {
                model.name.clone()
            } else {
                model.label.clone()
            }
        })
        .unwrap_or_else(|| "модель".to_owned())
}

/// Selector of `control` for `chat`, or `None` when there is nothing to
/// choose from.
pub(in crate::ui) fn selector(
    control: Control,
    config: &ConfigSummary,
    chat: WeakEntity<Chat>,
) -> Option<AnyElement> {
    match control {
        Control::Mode => Some(mode_selector(config, chat)),
        Control::Model => model_selector(config, chat),
        Control::Effort => effort_selector(config, chat),
    }
}

fn mode_selector(config: &ConfigSummary, chat: WeakEntity<Chat>) -> AnyElement {
    let mode = mode_from_value(&config.permission_mode);
    button("mode-selector", mode_label(&config.permission_mode))
        .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
            MODES.iter().fold(menu, |menu, (value, label)| {
                let (chat, value) = (chat.clone(), *value);
                menu.item(
                    PopupMenuItem::new(*label)
                        .checked(value == mode)
                        .on_click(move |_, _, cx| {
                            let _ = chat.update(cx, |chat, cx| chat.set_mode(value, cx));
                        }),
                )
            })
        })
        .into_any_element()
}

fn model_selector(config: &ConfigSummary, chat: WeakEntity<Chat>) -> Option<AnyElement> {
    let current = config.model.as_ref().map(|model| model.name.clone());
    let models = config
        .model_options
        .iter()
        .filter(|option| !option.hidden || Some(&option.name) == current.as_ref())
        .map(|option| {
            let label = if option.label.is_empty() {
                &option.name
            } else {
                &option.label
            };
            (option.name.clone(), label.clone())
        })
        .collect::<Vec<_>>();
    if models.is_empty() {
        return None;
    }
    let element = button("model-selector", model_label(config))
        .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu: PopupMenu, _, _| {
            models.iter().fold(
                menu.max_h(px(360.)).scrollable(true),
                |menu, (name, label)| {
                    let (chat, name) = (chat.clone(), name.clone());
                    let checked = Some(&name) == current.as_ref();
                    menu.item(PopupMenuItem::new(label.clone()).checked(checked).on_click(
                        move |_, _, cx| {
                            let name = name.clone();
                            let _ = chat.update(cx, |chat, cx| chat.set_model(name, cx));
                        },
                    ))
                },
            )
        })
        .into_any_element();
    Some(element)
}

fn effort_selector(config: &ConfigSummary, chat: WeakEntity<Chat>) -> Option<AnyElement> {
    let effort = current_effort(config);
    let efforts = std::iter::once(None)
        .chain(config.reasoning.effort_options.iter().cloned().map(Some))
        .collect::<Vec<_>>();
    if efforts.len() < 2 {
        return None;
    }
    let element = button("effort-selector", effort_label(effort.as_deref()))
        .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
            efforts.iter().fold(menu, |menu, value| {
                let (chat, value) = (chat.clone(), value.clone());
                let checked = value == effort;
                menu.item(
                    PopupMenuItem::new(effort_label(value.as_deref()))
                        .checked(checked)
                        .on_click(move |_, _, cx| {
                            let value = value.clone();
                            let _ = chat.update(cx, |chat, cx| chat.set_effort(value, cx));
                        }),
                )
            })
        })
        .into_any_element();
    Some(element)
}

fn button(id: &'static str, label: impl Into<SharedString>) -> Button {
    Button::new(id)
        .label(label)
        .ghost()
        .xsmall()
        .dropdown_caret(true)
}
