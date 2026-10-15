//! Packages page: Agent Plugins entries of the profile with their load
//! diagnostics and what they contribute; folders are added and removed
//! through the draft.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::domain::AgentPluginConfig;

use super::widgets::{Tone, card, changed_dot, chip, hashed_id, mono_line, note, section};
use super::{SettingsView, spot};
use crate::settings::addons::{package_contents, package_state};
use crate::settings::catalog::{PageId, plural};
use crate::ui::kit;
use crate::ui::kit::Hint as _;

impl SettingsView {
    pub(super) fn packages_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let mut parts: Vec<AnyElement> = self.addons_notice(cx).into_iter().collect();
        let packages = &self.draft.addons.addons.plugins;
        let rows: Vec<AnyElement> = if packages.is_empty() {
            vec![
                note(
                    "Пакетов нет. Добавьте папку пакета кнопкой «Добавить пакет…».",
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
        let add = kit::button("package-add")
            .icon(Lucide::FolderPlus)
            .label("Добавить пакет…")
            .ghost()
            .xsmall()
            .hint("Выбрать папку пакета Agent Plugins", cx)
            .on_click(cx.listener(|this, _, _, cx| this.choose_package(cx)));
        parts.push(
            section(
                format!("Пакеты · {enabled} из {}", packages.len()),
                Some(
                    "Пакет не запускает процессных модулей: он даёт skills и описания MCP-серверов."
                        .into(),
                ),
                Some(h_flex().flex_wrap().gap_2().child(add).child(
                    kit::button("package-zip").label("Из ZIP…").ghost().xsmall()
                        .disabled(self.catalogs.busy || self.saving)
                        .on_click(cx.listener(|this, _, _, cx| this.choose_package_zip(cx)))
                ).into_any_element()),
                card(rows, cx),
                cx,
            )
            .into_any_element(),
        );
        parts
    }

    /// Lists the picked folders in the draft; the save checks them.
    fn choose_package(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Добавить".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            this.update(cx, |this, cx| {
                this.update_draft(cx, |draft| {
                    for path in paths {
                        draft.add_package(path);
                    }
                })
            })
            .ok();
        })
        .detach();
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
                    plural(skills, "skill", "skills", "skills"),
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
        let removed = package.path.clone();
        let spot_key = spot::package(&package.path);
        // A folder added in the draft is loaded by the next save.
        let pending = self.addons().is_some() && state.is_none();
        let row = h_flex()
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
                    .when(pending, |text| {
                        text.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Пакет загрузится после сохранения."),
                        )
                    })
                    .when(!chips.is_empty(), |text| {
                        text.child(h_flex().flex_wrap().gap_1().items_center().children(chips))
                    }),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .items_center()
                    .child(
                        kit::button(hashed_id("package-remove", &path))
                            .icon(Lucide::Trash)
                            .ghost()
                            .xsmall()
                            .hint("Убрать пакет из профиля", cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_draft(cx, |draft| draft.remove_package(&removed))
                            })),
                    )
                    .child(
                        kit::switch(hashed_id("package", &path))
                            .checked(package.enabled)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                let checked = *checked;
                                this.update_draft(cx, |draft| draft.set_package(&target, checked));
                            })),
                    ),
            );
        self.spotted(&spot_key, row.into_any_element(), cx)
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
            .on_click(
                cx.listener(move |this, _, window, cx| this.select_page(page.clone(), window, cx)),
            )
            .into_any_element()
    }
}
