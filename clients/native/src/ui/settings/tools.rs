//! Tools page: which tools the profile allows.

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::Input;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::config_builder::ConfigBuilderTool;

use super::SettingsView;
use super::mcp::mcp_key;
use super::widgets::{Tone, card, changed_dot, chip, hashed_id, item_row, named, note, section};
use crate::settings::addons::tool_server;
use crate::settings::catalog::{PageId, missing_tools, tool_matches};
use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::prefs;

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
        let on = tool.runtime_managed || self.draft.tools.contains(&tool.name);
        let changed = !tool.runtime_managed
            && self.draft.tools.contains(&tool.name) != self.baseline.tools.contains(&tool.name);
        let mut chips = vec![chip(
            tool.safety.clone(),
            if tool.safety == "Dangerous" {
                Tone::Danger
            } else {
                Tone::Muted
            },
            cx,
        )];
        let server = self
            .addons()
            .and_then(|addons| tool_server(addons, &tool.name));
        if tool.runtime_managed && server.is_none() {
            chips.push(chip("управляется runtime", Tone::Accent, cx));
        }
        if !tool.registered {
            chips.push(chip("не зарегистрирован", Tone::Warning, cx));
        }
        let link = match (&tool.owner, server) {
            (Some(owner), _) => Some((
                owner.module_id.clone(),
                format!("Пакет модуля {} · открыть", owner.component_id),
                pack_key(&owner.component_id, &owner.module_id),
                PageId::Modules,
            )),
            (None, Some(server)) => Some((
                format!("MCP · {server}"),
                "Включается вместе с MCP-сервером".to_string(),
                mcp_key(server),
                PageId::Mcp,
            )),
            (None, None) => None,
        };
        let owner = link.map(|(label, hint, key, page)| {
            let view = cx.weak_entity();
            kit::button(hashed_id("tool-owner", &tool.name))
                .label(label)
                .ghost()
                .xsmall()
                .hint(&hint, cx)
                .on_click(move |_, window, cx| {
                    let (key, page) = (key.clone(), page.clone());
                    let _ = view.update(cx, |this, cx| {
                        this.open.insert(key);
                        this.select_page(page, window, cx);
                    });
                })
                .into_any_element()
        });

        let name = tool.name.clone();
        let switch = kit::switch(hashed_id("tool-switch", &tool.name))
            .checked(on)
            .disabled(tool.runtime_managed)
            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                let checked = *checked;
                this.update_draft(cx, |draft| draft.set_tool(&name, checked));
            }));
        let title = h_flex()
            .min_w_0()
            .gap_2()
            .items_center()
            .child(named(
                hashed_id("tool-name", &tool.name),
                tool.name.clone(),
                Some(tool.description.clone().into()),
                Some(format!("Источник: {}", tool.source).into()),
                cx,
            ))
            .when(changed, |line| line.child(changed_dot(cx)));
        let trailing = chips
            .into_iter()
            .map(IntoElement::into_any_element)
            .chain(owner)
            .chain([switch.into_any_element()]);
        self.spotted(
            &super::spot::tool(&tool.name),
            item_row(title, trailing).when(!tool.registered, |row| row.opacity(0.65)),
            cx,
        )
    }

    fn missing_tool_row(&self, name: &str, cx: &Context<Self>) -> AnyElement {
        let target = name.to_string();
        let switch = kit::switch(hashed_id("missing-tool", &name))
            .checked(true)
            .hint("Убрать из разрешённых", cx)
            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                let checked = *checked;
                this.update_draft(cx, |draft| draft.set_tool(&target, checked));
            }));
        item_row(
            named(
                hashed_id("missing-tool-name", &name),
                name.to_string(),
                None,
                None,
                cx,
            ),
            [
                chip("Не найден в текущей сборке", Tone::Danger, cx).into_any_element(),
                switch.into_any_element(),
            ],
        )
        .into_any_element()
    }
}
