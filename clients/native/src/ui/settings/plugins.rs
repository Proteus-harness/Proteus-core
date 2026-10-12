//! Plugins page: process components with their tool packs, hooks and slot
//! modules, plus Agent Plugins packages (skills and MCP).

use crate::ui::kit;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::config_builder::{
    ConfigBuilderPlugin, ConfigBuilderPluginExport, ConfigBuilderToolPack,
};

use super::SettingsView;
use super::tools::pack_key;
use super::widgets::{Tone, card, chip, disclosure, hashed_id, hint_text, mono, note, section};
use crate::settings::catalog::{
    HOOK_SLOT, PackState, PageId, TOOL_SLOT, export_selected, find_tool, pack_summary, plural,
    safety_label, slot_page, slot_title,
};
use crate::ui::kit::Hint as _;
use crate::ui::prefs;
use gpui_kit::assets::IconName as Lucide;

impl SettingsView {
    pub(super) fn plugins_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(snapshot) = self.snapshot() else {
            return Vec::new();
        };
        let mut parts: Vec<AnyElement> = snapshot
            .plugins
            .iter()
            .map(|plugin| self.plugin_card(plugin, cx))
            .collect();
        if parts.is_empty() {
            parts.push(
                card(
                    [note("В профиле нет процессных плагинов.", cx).into_any_element()],
                    cx,
                )
                .into_any_element(),
            );
        }
        if !self.draft.addons.addons.plugins.is_empty() {
            parts.push(self.agent_plugins(cx));
        }
        parts
    }

    fn plugin_card(&self, plugin: &ConfigBuilderPlugin, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let hooks: Vec<_> = plugin
            .exports
            .iter()
            .filter(|export| export.slot == HOOK_SLOT)
            .collect();
        let modules: Vec<_> = plugin
            .exports
            .iter()
            .filter(|export| export.slot != HOOK_SLOT && export.slot != TOOL_SLOT)
            .collect();
        let mut counts = Vec::new();
        if !plugin.tool_packs.is_empty() {
            counts.push(plural(
                plugin.tool_packs.len(),
                "пакет инструментов",
                "пакета инструментов",
                "пакетов инструментов",
            ));
        }
        if !hooks.is_empty() {
            counts.push(plural(
                hooks.len(),
                "обработчик",
                "обработчика",
                "обработчиков",
            ));
        }
        if !modules.is_empty() {
            counts.push(plural(
                modules.len(),
                "модуль слота",
                "модуля слотов",
                "модулей слотов",
            ));
        }
        let header = v_flex()
            .px_4()
            .py_3()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Icon::new(Lucide::Package)
                            .small()
                            .text_color(theme.muted_foreground),
                    )
                    .child(div().text_base().font_semibold().child(plugin.id.clone())),
            )
            .children(hint_text(
                plugin.description.clone().unwrap_or_default(),
                cx,
            ))
            .child(
                mono(plugin.command.clone(), cx)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .truncate(),
            )
            .when(!counts.is_empty(), |header| {
                header.child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .children(counts.into_iter().map(|text| chip(text, Tone::Muted, cx))),
                )
            });

        let mut rows = vec![header.into_any_element()];
        if plugin.exports.is_empty() && plugin.tool_packs.is_empty() {
            rows.push(note("Плагин не объявил exports.", cx).into_any_element());
        }
        if !plugin.tool_packs.is_empty() {
            rows.push(group_label("Пакеты инструментов", cx));
            rows.extend(
                plugin
                    .tool_packs
                    .iter()
                    .map(|pack| self.pack_row(plugin, pack, cx)),
            );
        }
        if !hooks.is_empty() {
            rows.push(group_label("Обработчики", cx));
            rows.extend(
                hooks
                    .into_iter()
                    .map(|export| self.hook_export_row(export, cx)),
            );
        }
        if !modules.is_empty() {
            rows.push(group_label("Модули слотов", cx));
            rows.extend(
                modules
                    .into_iter()
                    .map(|export| self.module_export_row(export, cx)),
            );
        }
        card(rows, cx).into_any_element()
    }

    fn pack_row(
        &self,
        plugin: &ConfigBuilderPlugin,
        pack: &ConfigBuilderToolPack,
        cx: &Context<Self>,
    ) -> AnyElement {
        let snapshot = self.snapshot().expect("plugins page needs a snapshot");
        let summary = pack_summary(snapshot, &self.draft, pack);
        let key = pack_key(&plugin.id, &pack.id);
        let open = self.is_open(&key);
        let export = plugin
            .exports
            .iter()
            .find(|export| export.slot == TOOL_SLOT && export.id == pack.id);
        let changed = pack
            .tools
            .iter()
            .any(|name| self.draft.tools.contains(name) != self.baseline.tools.contains(name))
            || self.draft.module_config(TOOL_SLOT, &pack.id)
                != self.baseline.module_config(TOOL_SLOT, &pack.id);
        let view = cx.weak_entity();
        let toggle_key = key.clone();
        let title = format!("{} · {} из {}", pack.id, summary.enabled, summary.total);
        let line = disclosure(
            hashed_id("pack", &key),
            open,
            title,
            changed,
            move |cx| {
                let _ = view.update(cx, |this, cx| this.toggle_open(&toggle_key, cx));
            },
            cx,
        );
        let switchable = summary.switchable.clone();
        let target = summary.state != PackState::On;
        let switch = kit::switch(hashed_id("pack-switch", &key))
            .checked(summary.state != PackState::Off)
            .disabled(switchable.is_empty())
            .hint(
                match summary.state {
                    PackState::On => "Выключить пакет",
                    PackState::Off => "Включить пакет",
                    PackState::Mixed => "Включить все",
                },
                cx,
            )
            .on_click(cx.listener(move |this, _: &bool, _, cx| {
                cx.stop_propagation();
                let names = switchable.clone();
                this.update_draft(cx, |draft| {
                    for name in &names {
                        draft.set_tool(name, target);
                    }
                });
            }));
        // The whole row is one hover target, so the switch sits inside it.
        let header = line
            .pr_4()
            .child(div().flex_1())
            .when(summary.state == PackState::Mixed, |row| {
                row.child(chip("частично", Tone::Warning, cx))
            })
            .child(switch);

        let content = self.reveal(&key, cx, || {
            let mut content = v_flex().children(
                export
                    .and_then(|export| export.description.clone())
                    .and_then(|text| hint_text(text, cx))
                    .map(|hint| hint.px_4().pb_2()),
            );
            if pack.tools.is_empty() {
                content = content.child(note("Пакет не сообщил инструментов.", cx));
            }
            for name in &pack.tools {
                content = content.child(self.pack_tool_row(name, cx));
            }
            if export.is_some() {
                let params_key = format!("params:{TOOL_SLOT}:{}", pack.id);
                let params_open = self.is_open(&params_key);
                content = content.child(self.params_disclosure(
                    TOOL_SLOT,
                    &pack.id,
                    &params_key,
                    params_open,
                    cx,
                ));
            }
            content.into_any_element()
        });
        v_flex().child(header).children(content).into_any_element()
    }

    fn pack_tool_row(&self, name: &str, cx: &Context<Self>) -> AnyElement {
        let snapshot = self.snapshot().expect("plugins page needs a snapshot");
        let tool = find_tool(snapshot, name);
        let runtime = tool.is_some_and(|tool| tool.runtime_managed);
        let mut chips = Vec::new();
        if let Some(tool) = tool {
            chips.push(chip(
                safety_label(&tool.safety).to_string(),
                Tone::Muted,
                cx,
            ));
        } else {
            chips.push(chip("нет в списке инструментов", Tone::Warning, cx));
        }
        if runtime {
            chips.push(chip("управляется runtime", Tone::Accent, cx));
        }
        let target = name.to_string();
        h_flex()
            .pl(prefs::scaled(40.))
            .pr_4()
            .py_1p5()
            .gap_2()
            .items_center()
            .child(
                mono(name.to_string(), cx)
                    .text_sm()
                    .flex_1()
                    .min_w_0()
                    .truncate(),
            )
            .children(chips)
            .child(
                kit::switch(hashed_id("pack-tool", &name))
                    .checked(runtime || self.draft.tools.contains(name))
                    .disabled(runtime || tool.is_none())
                    .small()
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        let checked = *checked;
                        this.update_draft(cx, |draft| draft.set_tool(&target, checked));
                    })),
            )
            .into_any_element()
    }

    fn hook_export_row(
        &self,
        export: &ConfigBuilderPluginExport,
        cx: &Context<Self>,
    ) -> AnyElement {
        let position = self.draft.hooks.iter().position(|hook| hook == &export.id);
        let key = format!("params:{HOOK_SLOT}:{}", export.id);
        let open = self.is_open(&key);
        let (id, on) = (export.id.clone(), position.is_none());
        let mut chips = vec![chip("Hooks", Tone::Muted, cx)];
        chips.push(match position {
            Some(ix) => chip(format!("№ {} в цепочке", ix + 1), Tone::Success, cx),
            None => chip("выключен", Tone::Muted, cx),
        });
        let order = position.is_some().then(|| {
            kit::button(hashed_id("hook-order", &export.id))
                .label("Порядок")
                .ghost()
                .xsmall()
                .on_click(cx.listener(|this, _, _, cx| this.select_page(PageId::Hooks, cx)))
        });
        let header = export_header(export, chips, cx).children(order).child(
            kit::switch(hashed_id("hook-switch", &export.id))
                .checked(position.is_some())
                .on_click(cx.listener(move |this, _: &bool, _, cx| {
                    this.update_draft(cx, |draft| draft.set_hook(&id, on))
                })),
        );
        v_flex()
            .child(header)
            .child(self.params_disclosure(HOOK_SLOT, &export.id, &key, open, cx))
            .into_any_element()
    }

    fn module_export_row(
        &self,
        export: &ConfigBuilderPluginExport,
        cx: &Context<Self>,
    ) -> AnyElement {
        let snapshot = self.snapshot().expect("plugins page needs a snapshot");
        let title = slot_title(snapshot, &export.slot);
        let mut chips = vec![chip(title.clone(), Tone::Muted, cx)];
        if export_selected(snapshot, &self.draft, export) {
            chips.push(chip("выбран", Tone::Success, cx));
        }
        let page = slot_page(&export.slot);
        let open = kit::button(hashed_id("open-slot", &(&export.slot, &export.id)))
            .label(format!("Открыть {title}"))
            .ghost()
            .xsmall()
            .on_click(cx.listener(move |this, _, _, cx| this.select_page(page.clone(), cx)));
        export_header(export, chips, cx)
            .child(open)
            .into_any_element()
    }

    fn agent_plugins(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let rows = self
            .draft
            .addons
            .addons
            .plugins
            .iter()
            .enumerate()
            .map(|(ix, plugin)| {
                let path = plugin.path.display().to_string();
                h_flex()
                    .px_4()
                    .py_3()
                    .gap_3()
                    .items_center()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(mono(path, cx).text_sm().truncate())
                            .child(h_flex().child(chip("skills и MCP", Tone::Muted, cx))),
                    )
                    .child(
                        kit::switch(("agent-plugin", ix))
                            .checked(plugin.enabled)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                let checked = *checked;
                                this.update_draft(cx, |draft| {
                                    if let Some(plugin) = draft.addons.addons.plugins.get_mut(ix) {
                                        plugin.enabled = checked;
                                    }
                                });
                            })),
                    )
                    .into_any_element()
            });
        section(
            "Пакеты Agent Plugins",
            Some(
                "Пакеты навыков и MCP-серверов. Это не процессные плагины: они не дают tools exports и обработчиков."
                    .into(),
            ),
            None,
            card(rows, cx),
            cx,
        )
        .text_color(theme.foreground)
        .into_any_element()
    }
}

fn group_label(text: &'static str, cx: &App) -> AnyElement {
    div()
        .px_4()
        .pt_3()
        .pb_1()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

fn export_header(export: &ConfigBuilderPluginExport, chips: Vec<Div>, cx: &App) -> Div {
    h_flex().px_4().py_2p5().gap_3().items_center().child(
        v_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(mono(export.id.clone(), cx).text_sm())
            .children(hint_text(
                export.description.clone().unwrap_or_default(),
                cx,
            ))
            .child(h_flex().flex_wrap().gap_1().children(chips)),
    )
}
