//! Packages page: Agent Plugins entries of the profile with their load
//! diagnostics and what they contribute.

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::domain::AgentPluginConfig;

use super::SettingsView;
use super::widgets::{Tone, card, changed_dot, chip, hashed_id, mono_line, note, section};
use crate::settings::addons::{package_contents, package_state};
use crate::settings::catalog::{PageId, plural};
use crate::ui::kit;

impl SettingsView {
    pub(super) fn packages_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let mut parts: Vec<AnyElement> = self.addons_notice(cx).into_iter().collect();
        let packages = &self.draft.addons.addons.plugins;
        let rows: Vec<AnyElement> = if packages.is_empty() {
            vec![
                note(
                    "Пакетов нет. Их подключают записи [[addons.plugins]] профиля.",
                    cx,
                )
                .into_any_element(),
            ]
        } else {
            packages
                .iter()
                .map(|package| self.package_row(package, cx))
                .collect()
        };
        let enabled = packages.iter().filter(|package| package.enabled).count();
        parts.push(
            section(
                format!("Пакеты · {enabled} из {}", packages.len()),
                Some(
                    "Пакет не запускает процессных модулей: он даёт навыки и описания MCP-серверов."
                        .into(),
                ),
                None,
                card(rows, cx),
                cx,
            )
            .into_any_element(),
        );
        parts
    }

    fn package_row(&self, package: &AgentPluginConfig, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let path = package.path.display().to_string();
        let state = self
            .addons()
            .and_then(|addons| package_state(addons, &package.path));
        let name = state.and_then(|state| state.name.clone());
        let changed = self
            .baseline
            .addons
            .addons
            .plugins
            .iter()
            .find(|entry| entry.path == package.path)
            .is_none_or(|entry| entry.enabled != package.enabled);
        let mut chips: Vec<AnyElement> = state
            .and_then(|state| state.version.clone())
            .map(|version| chip(version, Tone::Muted, cx).into_any_element())
            .into_iter()
            .collect();
        if let (Some(addons), Some(name)) = (self.addons(), &name) {
            let (skills, servers) = package_contents(addons, name);
            if skills > 0 {
                chips.push(self.page_link(
                    ("package-skills", name),
                    plural(skills, "навык", "навыка", "навыков"),
                    PageId::Skills,
                    cx,
                ));
            }
            if servers > 0 {
                chips.push(self.page_link(
                    ("package-mcp", name),
                    plural(servers, "MCP-сервер", "MCP-сервера", "MCP-серверов"),
                    PageId::Mcp,
                    cx,
                ));
            }
        }
        let problems = state.into_iter().flat_map(|state| {
            state
                .error
                .iter()
                .map(|error| (theme.danger, error.clone()))
                .chain(
                    state
                        .warnings
                        .iter()
                        .map(|warning| (theme.warning, warning.clone())),
                )
        });
        let target = package.path.clone();
        h_flex()
            .px_4()
            .py_3()
            .gap_4()
            .items_start()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_sm()
                            .child(match &name {
                                Some(name) => div().font_semibold().child(name.clone()),
                                None => mono_line(path.clone(), cx),
                            })
                            .when(changed, |line| line.child(changed_dot(cx))),
                    )
                    .children(state.and_then(|state| state.description.clone()).map(
                        |description| {
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(description)
                        },
                    ))
                    .when(name.is_some(), |text| {
                        text.child(
                            mono_line(path.clone(), cx)
                                .text_xs()
                                .text_color(theme.muted_foreground),
                        )
                    })
                    .children(
                        problems.map(|(color, text)| div().text_xs().text_color(color).child(text)),
                    )
                    .when(!chips.is_empty(), |text| {
                        text.child(h_flex().flex_wrap().gap_1().items_center().children(chips))
                    }),
            )
            .child(
                div().pt_0p5().child(
                    kit::switch(hashed_id("package", &path))
                        .checked(package.enabled)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            let checked = *checked;
                            this.update_draft(cx, |draft| draft.set_package(&target, checked));
                        })),
                ),
            )
            .into_any_element()
    }

    fn page_link(
        &self,
        key: (&'static str, &str),
        label: String,
        page: PageId,
        cx: &Context<Self>,
    ) -> AnyElement {
        kit::button(hashed_id(key.0, &key.1))
            .label(label)
            .ghost()
            .xsmall()
            .on_click(cx.listener(move |this, _, _, cx| this.select_page(page.clone(), cx)))
            .into_any_element()
    }
}
