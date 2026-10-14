//! MCP page: servers of the profile and of packages. Profile servers are
//! added and edited in place (`mcp_form`), package servers only switch on
//! and off; tools and errors come from the discovery of the saved assembly.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::*;
use gpui_kit::*;

use super::addons::AddonsState;
use super::widgets::{Tone, card, chip, disclosure, hashed_id, mono_line, note, section};
use super::{SettingsView, spot};
use crate::settings::addons::{McpServer, mcp_servers};
use crate::settings::catalog::{find_tool, plural};
use crate::settings::mcp::mcp_problems;
use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::prefs;

/// Disclosure key of a server, also opened from the Tools page.
pub(super) fn mcp_key(server: &str) -> String {
    format!("mcp:{server}")
}

/// Disclosure key of the folded fields of a profile server.
pub(super) fn mcp_advanced_key(server: &str) -> String {
    format!("{}:advanced", mcp_key(server))
}

impl SettingsView {
    pub(super) fn mcp_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let mut parts: Vec<AnyElement> = self.addons_notice(cx).into_iter().collect();
        let servers = mcp_servers(&self.draft, self.addons());
        if servers.is_empty() && matches!(self.addons, AddonsState::Loading { .. }) {
            return parts;
        }
        let enabled = servers
            .iter()
            .filter(|server| self.draft.mcp_enabled(server.name))
            .count();
        let rows: Vec<AnyElement> = if servers.is_empty() {
            vec![
                note(
                    "Серверов нет. Добавьте свой сервер или подключите пакет со страницы Packages.",
                    cx,
                )
                .into_any_element(),
            ]
        } else {
            servers
                .iter()
                .map(|server| self.mcp_row(server, cx))
                .collect()
        };
        let add = kit::button("mcp-add")
            .icon(Lucide::Plus)
            .label("Добавить сервер")
            .ghost()
            .xsmall()
            .hint("Новая запись [[tools.mcp_servers]] профиля", cx)
            .on_click(cx.listener(|this, _, _, cx| this.add_mcp_server(cx)));
        parts.push(
            section(
                format!("Серверы · {enabled} из {}", servers.len()),
                Some(
                    "Включённый сервер запускается при сборке агента и отдаёт ему свои инструменты."
                        .into(),
                ),
                Some(add.into_any_element()),
                card(rows, cx),
                cx,
            )
            .into_any_element(),
        );
        parts
    }

    /// Appends a profile server and opens its form.
    fn add_mcp_server(&mut self, cx: &mut Context<Self>) {
        let mut name = String::new();
        self.update_draft(cx, |draft| {
            let ix = draft.add_mcp();
            name = draft.addons.mcp_servers[ix].name.clone();
        });
        let key = mcp_key(&name);
        if !self.is_open(&key) {
            self.toggle_open(&key, cx);
        }
    }

    fn mcp_row(&self, server: &McpServer, cx: &Context<Self>) -> AnyElement {
        let key = mcp_key(server.name);
        let open = self.is_open(&key);
        let on = self.draft.mcp_enabled(server.name);
        let changed = on != self.baseline.mcp_enabled(server.name)
            || server.configured.is_some_and(|(ix, configured)| {
                self.draft.mcp_baseline(&self.baseline, ix) != Some(configured)
            });
        let origin = match (server.configured, server.package) {
            (Some(_), _) => "профиль".to_string(),
            (None, Some(package)) => format!("пакет {package}"),
            (None, None) => "пакет".to_string(),
        };
        let mut chips = vec![chip(origin, Tone::Muted, cx)];
        let invalid = server.configured.is_some_and(|(ix, _)| {
            mcp_problems(&self.draft, self.addons())
                .iter()
                .any(|problem| problem.index == ix)
        });
        if invalid {
            chips.push(chip("нужно исправить", Tone::Danger, cx));
        }
        match server.state {
            Some(state) if state.error.is_some() => chips.push(chip("ошибка", Tone::Danger, cx)),
            Some(state) if state.enabled => chips.push(chip(
                plural(
                    state.tools.len(),
                    "инструмент",
                    "инструмента",
                    "инструментов",
                ),
                Tone::Success,
                cx,
            )),
            // Off in the saved assembly though the profile allows it.
            Some(_) if self.baseline.mcp_enabled(server.name) => chips.push(chip(
                server.package.map_or("не запущен".to_string(), |package| {
                    format!("пакет {package} выключен")
                }),
                Tone::Warning,
                cx,
            )),
            _ => {}
        }
        let view = cx.weak_entity();
        let toggle_key = key.clone();
        let line = disclosure(
            hashed_id("mcp", &key),
            open,
            server.name.to_string(),
            changed,
            move |cx| {
                let _ = view.update(cx, |this, cx| this.toggle_open(&toggle_key, cx));
            },
            cx,
        );
        let name = server.name.to_string();
        let switch = kit::switch(hashed_id("mcp-switch", &key))
            .checked(on)
            .hint(
                if on {
                    "Выключить сервер"
                } else {
                    "Включить сервер"
                },
                cx,
            )
            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                cx.stop_propagation();
                let checked = *checked;
                this.update_draft(cx, |draft| draft.set_mcp(&name, checked));
            }));
        // The whole row is one hover target, so the switch sits inside it.
        let header = line
            .pr_4()
            .child(div().flex_1())
            .children(chips)
            .child(switch);
        let content = self.reveal(&key, cx, || self.mcp_details(server, cx));
        v_flex()
            .child(self.spotted(&spot::mcp(server.name), header.into_any_element(), cx))
            .children(content)
            .into_any_element()
    }

    fn mcp_details(&self, server: &McpServer, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let mut content = v_flex().children(
            server
                .configured
                .map(|(ix, configured)| self.mcp_form(ix, configured, cx)),
        );
        let Some(state) = server.state else {
            return content
                .child(note("Сервер появится в сборке после сохранения.", cx))
                .into_any_element();
        };
        if let Some(error) = &state.error {
            content = content.child(
                div()
                    .px_4()
                    .pb_2()
                    .text_sm()
                    .text_color(theme.danger)
                    .child(error.clone()),
            );
        } else if !state.enabled {
            content = content.child(note(
                "Сервер выключен: сборка его не запускала, инструменты неизвестны.",
                cx,
            ));
        } else if state.tools.is_empty() {
            content = content.child(note("Сервер не сообщил инструментов.", cx));
        } else if server.configured.is_some() {
            content = content.child(
                div()
                    .px_4()
                    .py_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Инструменты сохранённой сборки"),
            );
        }
        let snapshot = self.snapshot();
        for name in &state.tools {
            let safety = snapshot
                .and_then(|snapshot| find_tool(snapshot, name))
                .map(|tool| chip(tool.safety.clone(), Tone::Muted, cx));
            content = content.child(
                h_flex()
                    .pl(prefs::scaled(40.))
                    .pr_4()
                    .py_1p5()
                    .gap_2()
                    .items_center()
                    .child(mono_line(name.clone(), cx).text_sm().flex_1())
                    .children(safety),
            );
        }
        content.into_any_element()
    }
}
