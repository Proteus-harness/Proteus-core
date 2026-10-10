use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::*;
use gpui_kit::*;

use super::SettingsView;
use crate::settings::{Choice, Draft, Group, Item, Page};
use crate::ui::controls::mode_label;

const TOOL_DESCRIPTION_CHARS: usize = 160;

impl SettingsView {
    pub(super) fn setting_pages(&self, pages: &[Page], cx: &Context<Self>) -> Vec<SettingPage> {
        let view = cx.weak_entity();
        pages
            .iter()
            .map(|page| {
                let mut setting = SettingPage::new(page.title.clone()).resettable(false);
                if let Some(description) = &page.description {
                    setting = setting.description(description.clone());
                }
                setting.groups(page.groups.iter().map(|group| self.group(group, &view)))
            })
            .collect()
    }

    fn group(&self, group: &Group, view: &WeakEntity<Self>) -> SettingGroup {
        let mut setting = SettingGroup::new();
        if let Some(title) = &group.title {
            setting = setting.title(title.clone());
        }
        if let Some(description) = &group.description {
            setting = setting.description(description.clone());
        }
        setting.items(group.items.iter().map(|item| self.item(item, view)))
    }

    fn item(&self, item: &Item, view: &WeakEntity<Self>) -> SettingItem {
        match item {
            Item::Provider(choices) => {
                let current = self.draft.provider.clone().unwrap_or_default();
                dropdown(
                    "Провайдер",
                    choices,
                    current,
                    view,
                    |draft, value| {
                        draft.provider = Some(value);
                    },
                )
                .description("Модель, которой агент отправляет запросы.")
            }
            Item::Mode(modes) => {
                let choices: Vec<Choice> = modes
                    .iter()
                    .map(|mode| Choice {
                        value: mode.clone(),
                        label: mode_label(mode).to_string(),
                    })
                    .collect();
                let current = self.draft.mode.clone();
                dropdown(
                    "Режим доступа",
                    &choices,
                    current,
                    view,
                    |draft, value| {
                        draft.mode = value;
                    },
                )
                .description("Режим по умолчанию для новых запусков этого профиля.")
            }
            Item::Module { slot, options } => {
                let current = self.draft.modules.get(slot).cloned().unwrap_or_default();
                let slot = slot.clone();
                dropdown(
                    "Реализация",
                    options,
                    current,
                    view,
                    move |draft, value| {
                        draft.set_module(&slot, &value);
                    },
                )
            }
            Item::Hook { id, description } => {
                let enabled = self.draft.hooks.contains(id);
                let (id, view) = (id.clone(), view.clone());
                let item = SettingItem::new(
                    id.clone(),
                    SettingField::switch(
                        move |_| enabled,
                        move |on, cx| {
                            let id = id.clone();
                            let _ = view.update(cx, |this, cx| {
                                this.update_draft(cx, |draft| draft.set_hook(&id, on));
                            });
                        },
                    ),
                );
                match description {
                    Some(description) => item.description(SharedString::from(description.clone())),
                    None => item,
                }
            }
            Item::Tool {
                name,
                description,
                locked,
            } => {
                let enabled = self.draft.tools.contains(name);
                let (tool, view) = (name.clone(), view.clone());
                SettingItem::new(
                    name.clone(),
                    SettingField::switch(
                        move |_| enabled,
                        move |on, cx| {
                            let tool = tool.clone();
                            let _ = view.update(cx, |this, cx| {
                                this.update_draft(cx, |draft| draft.set_tool(&tool, on));
                            });
                        },
                    ),
                )
                .description(SharedString::from(first_line(description)))
                .keywords([name.clone()])
                .disabled(*locked)
            }
            Item::Field { path, field } => self.field_item(path, field, view),
            Item::Note(text) => {
                let text = SharedString::from(text.clone());
                SettingItem::render(move |_, _, cx| {
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(text.clone())
                })
            }
        }
    }
}

fn dropdown(
    title: &'static str,
    choices: &[Choice],
    current: String,
    view: &WeakEntity<SettingsView>,
    apply: impl Fn(&mut Draft, String) + 'static,
) -> SettingItem {
    let options = choices
        .iter()
        .map(|choice| {
            (
                SharedString::from(choice.value.clone()),
                SharedString::from(choice.label.clone()),
            )
        })
        .collect();
    let current = SharedString::from(current);
    let view = view.clone();
    let apply = std::rc::Rc::new(apply);
    SettingItem::new(
        title,
        SettingField::scrollable_dropdown(
            options,
            move |_| current.clone(),
            move |value, cx| {
                let apply = apply.clone();
                let _ = view.update(cx, |this, cx| {
                    this.update_draft(cx, |draft| apply(draft, value.to_string()));
                });
            },
        ),
    )
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let mut clipped: String = line.trim().chars().take(TOOL_DESCRIPTION_CHARS).collect();
    if clipped.chars().count() < line.trim().chars().count() {
        clipped.push('…');
    }
    clipped
}
