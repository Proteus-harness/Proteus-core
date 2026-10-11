//! Tools page: which tools the profile allows.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::config_builder::ConfigBuilderTool;

use super::SettingsView;
use super::widgets::{Tone, card, changed_dot, chip, hashed_id, mono, note, section};
use crate::settings::catalog::{PageId, missing_tools, safety_label, tool_matches};
use crate::ui::prefs;
use crate::ui::tip::Tip as _;

const DESCRIPTION_FOLD: usize = 160;

pub(super) fn pack_key(component: &str, module: &str) -> String {
    format!("pack:{component}:{module}")
}

impl SettingsView {
    pub(super) fn tools_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(snapshot) = self.snapshot() else {
            return Vec::new();
        };
        let enabled = snapshot
            .tools
            .iter()
            .filter(|tool| tool.runtime_managed || self.draft.tools.contains(&tool.name))
            .count();
        let mut rows: Vec<AnyElement> = snapshot
            .tools
            .iter()
            .filter(|tool| tool_matches(tool, &self.tool_query))
            .map(|tool| self.tool_row(tool, cx))
            .collect();
        rows.extend(
            missing_tools(snapshot, &self.draft)
                .into_iter()
                .filter(|name| name.contains(self.tool_query.as_str()))
                .map(|name| self.missing_tool_row(name, cx)),
        );
        if rows.is_empty() {
            rows.push(note("Ничего не найдено.", cx).into_any_element());
        }
        let search = Input::new(&self.tool_search)
            .small()
            .cleanable(true)
            .prefix(Icon::new(IconName::Search).small())
            .w(prefs::scaled(260.));
        vec![
            section(
                format!(
                    "Разрешённые инструменты · {enabled} из {}",
                    snapshot.tools.len()
                ),
                None,
                Some(search.into_any_element()),
                card(rows, cx),
                cx,
            )
            .into_any_element(),
        ]
    }

    fn tool_row(&self, tool: &ConfigBuilderTool, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let on = tool.runtime_managed || self.draft.tools.contains(&tool.name);
        let changed = !tool.runtime_managed
            && self.draft.tools.contains(&tool.name) != self.baseline.tools.contains(&tool.name);
        let mut chips = vec![chip(
            safety_label(&tool.safety).to_string(),
            if tool.safety == "Dangerous" {
                Tone::Danger
            } else {
                Tone::Muted
            },
            cx,
        )];
        if tool.runtime_managed {
            chips.push(chip("управляется runtime", Tone::Accent, cx));
        }
        if !tool.registered {
            chips.push(chip("не зарегистрирован", Tone::Warning, cx));
        }
        let owner = tool.owner.as_ref().map(|owner| {
            let key = pack_key(&owner.component_id, &owner.module_id);
            let view = cx.weak_entity();
            Button::new(hashed_id("tool-owner", &tool.name))
                .label(format!("{} · {}", owner.component_id, owner.module_id))
                .ghost()
                .xsmall()
                .tooltip("Открыть пакет в Plugins")
                .on_click(move |_, _, cx| {
                    let key = key.clone();
                    let _ = view.update(cx, |this, cx| {
                        this.open.insert(key);
                        this.select_page(PageId::Plugins, cx);
                    });
                })
        });

        let description = tool.description.trim();
        let fold_key = format!("tool-description:{}", tool.name);
        let long = description.chars().count() > DESCRIPTION_FOLD;
        let expanded = self.is_open(&fold_key);
        let shown = if long && !expanded {
            let cut: String = description.chars().take(DESCRIPTION_FOLD).collect();
            format!("{}…", cut.trim_end())
        } else {
            description.to_string()
        };
        let more = long.then(|| {
            Button::new(hashed_id("tool-more", &tool.name))
                .label(if expanded {
                    "Свернуть"
                } else {
                    "Подробнее"
                })
                .link()
                .xsmall()
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_open(&fold_key, cx)))
        });

        let name = tool.name.clone();
        let source = format!("Источник: {}", tool.source);
        let switch = Switch::new(hashed_id("tool-switch", &tool.name))
            .checked(on)
            .disabled(tool.runtime_managed)
            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                let checked = *checked;
                this.update_draft(cx, |draft| draft.set_tool(&name, checked));
            }));
        h_flex()
            .px_4()
            .py_3()
            .gap_4()
            .items_start()
            .when(!tool.registered, |row| row.opacity(0.65))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .id(hashed_id("tool-name", &tool.name))
                            .gap_2()
                            .items_center()
                            .child(mono(tool.name.clone(), cx).text_sm())
                            .when(changed, |line| line.child(changed_dot(cx)))
                            .tip_text(source),
                    )
                    .when(!shown.is_empty(), |text| {
                        text.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(shown),
                        )
                    })
                    .children(more.map(|button| h_flex().child(button)))
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_1()
                            .items_center()
                            .children(chips)
                            .children(owner),
                    ),
            )
            .child(div().pt_0p5().child(switch))
            .into_any_element()
    }

    fn missing_tool_row(&self, name: &str, cx: &Context<Self>) -> AnyElement {
        let target = name.to_string();
        h_flex()
            .px_4()
            .py_3()
            .gap_4()
            .items_center()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(mono(name.to_string(), cx).text_sm())
                    .child(h_flex().child(chip("Не найден в текущей сборке", Tone::Danger, cx))),
            )
            .child(
                Switch::new(hashed_id("missing-tool", &name))
                    .checked(true)
                    .tooltip("Убрать из списка разрешённых")
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        let checked = *checked;
                        this.update_draft(cx, |draft| draft.set_tool(&target, checked));
                    })),
            )
            .into_any_element()
    }
}
