//! MCP page: servers of the profile and of packages. The switch edits the
//! draft; tools and errors come from the discovery of the saved assembly.

use gpui_kit::component::*;
use gpui_kit::*;

use super::SettingsView;
use super::widgets::{Tone, card, chip, disclosure, hashed_id, mono_line, note, section};
use crate::settings::addons::{McpServer, mcp_servers};
use crate::settings::catalog::{find_tool, plural};
use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::prefs;

/// Disclosure key of a server, also opened from the Tools page.
pub(super) fn mcp_key(server: &str) -> String {
    format!("mcp:{server}")
}

impl SettingsView {
    pub(super) fn mcp_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let mut parts: Vec<AnyElement> = self.addons_notice(cx).into_iter().collect();
        let servers = mcp_servers(&self.draft, self.addons());
        if servers.is_empty() && self.addons().is_none() {
            return parts;
        }
        let enabled = servers
            .iter()
            .filter(|server| self.draft.mcp_enabled(server.name))
            .count();
        let rows: Vec<AnyElement> = if servers.is_empty() {
            vec![
                note(
                    "Серверов нет. Их задают записи [[tools.mcp_servers]] профиля и пакеты со страницы Packages.",
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
        parts.push(
            section(
                format!("Серверы · {enabled} из {}", servers.len()),
                Some(
                    "Включённый сервер запускается при сборке агента и отдаёт ему свои инструменты."
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

    fn mcp_row(&self, server: &McpServer, cx: &Context<Self>) -> AnyElement {
        let key = mcp_key(server.name);
        let open = self.is_open(&key);
        let on = self.draft.mcp_enabled(server.name);
        let changed = on != self.baseline.mcp_enabled(server.name);
        let origin = match (server.configured, server.package) {
            (Some(_), _) => "профиль".to_string(),
            (None, Some(package)) => format!("пакет {package}"),
            (None, None) => "пакет".to_string(),
        };
        let mut chips = vec![chip(origin, Tone::Muted, cx)];
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
        v_flex().child(header).children(content).into_any_element()
    }

    fn mcp_details(&self, server: &McpServer, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let mut content = v_flex().children(server.configured.map(|configured| {
            let command = std::iter::once(configured.command.as_str())
                .chain(configured.args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ");
            div()
                .px_4()
                .pb_2()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(mono_line(command, cx))
        }));
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
