use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::app_protocol::config::ConfigSummary;
use proteus_contracts::domain::PermissionMode;

use super::workspace::Workspace;

const MODES: [(PermissionMode, &str); 3] = [
    (PermissionMode::Normal, "По правилам"),
    (PermissionMode::Auto, "Правки без вопросов"),
    (PermissionMode::Plan, "Планирование"),
];

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

impl Workspace {
    pub(super) fn render_controls(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let config = self.session.as_ref()?.config.as_ref()?;
        let this = cx.entity().downgrade();

        let mode = mode_from_value(&config.permission_mode);
        let mode_label = MODES
            .iter()
            .find(|(value, _)| *value == mode)
            .map_or("По правилам", |(_, label)| label);
        let mode_button =
            selector("mode", mode_label).dropdown_menu_with_anchor(Anchor::BottomLeft, {
                let this = this.clone();
                move |menu, _, _| {
                    MODES.iter().fold(menu, |menu, (value, label)| {
                        let (this, value) = (this.clone(), *value);
                        menu.item(PopupMenuItem::new(*label).checked(value == mode).on_click(
                            move |_, _, cx| {
                                let _ = this.update(cx, |this, cx| this.set_mode(value, cx));
                            },
                        ))
                    })
                }
            });

        let current_model = config.model.as_ref().map(|model| model.name.clone());
        let model_label = config
            .model
            .as_ref()
            .map(|model| {
                if model.label.is_empty() {
                    model.name.clone()
                } else {
                    model.label.clone()
                }
            })
            .unwrap_or_else(|| "модель".to_owned());
        let models = config
            .model_options
            .iter()
            .filter(|option| !option.hidden || Some(&option.name) == current_model.as_ref())
            .map(|option| {
                let label = if option.label.is_empty() {
                    &option.name
                } else {
                    &option.label
                };
                (option.name.clone(), label.clone())
            })
            .collect::<Vec<_>>();
        let model_button = (!models.is_empty()).then(|| {
            let this = this.clone();
            selector("model", model_label).dropdown_menu_with_anchor(
                Anchor::BottomLeft,
                move |menu: PopupMenu, _, _| {
                    models.iter().fold(
                        menu.max_h(px(360.)).scrollable(true),
                        |menu, (name, label)| {
                            let (this, name) = (this.clone(), name.clone());
                            let checked = Some(&name) == current_model.as_ref();
                            menu.item(PopupMenuItem::new(label.clone()).checked(checked).on_click(
                                move |_, _, cx| {
                                    let name = name.clone();
                                    let _ = this.update(cx, |this, cx| this.set_model(name, cx));
                                },
                            ))
                        },
                    )
                },
            )
        });

        let effort = current_effort(config);
        let efforts = std::iter::once(None)
            .chain(config.reasoning.effort_options.iter().cloned().map(Some))
            .collect::<Vec<_>>();
        let effort_button = (efforts.len() > 1).then(|| {
            selector("effort", effort_label(effort.as_deref())).dropdown_menu_with_anchor(
                Anchor::BottomLeft,
                move |menu, _, _| {
                    efforts.iter().fold(menu, |menu, value| {
                        let (this, value) = (this.clone(), value.clone());
                        let checked = value == effort;
                        menu.item(
                            PopupMenuItem::new(effort_label(value.as_deref()))
                                .checked(checked)
                                .on_click(move |_, _, cx| {
                                    let value = value.clone();
                                    let _ = this.update(cx, |this, cx| this.set_effort(value, cx));
                                }),
                        )
                    })
                },
            )
        });

        Some(
            h_flex()
                .gap_1()
                .child(mode_button)
                .children(model_button)
                .children(effort_button),
        )
    }
}

fn selector(id: &'static str, label: impl Into<SharedString>) -> Button {
    Button::new(id)
        .label(label)
        .ghost()
        .xsmall()
        .dropdown_caret(true)
}
