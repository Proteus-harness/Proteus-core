//! Modules page: process components of the profile with their tool packs,
//! hooks and slot implementations.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::config_builder::{
    ConfigBuilderPlugin, ConfigBuilderPluginExport, ConfigBuilderToolPack,
};

use super::SettingsView;
use super::tools::pack_key;
use super::widgets::{
    Tone, card, chip, described, disclosure, hashed_id, hint_text, item_row, named, note,
};
use crate::settings::catalog::{
    HOOK_SLOT, PackState, PageId, TOOL_SLOT, export_selected, find_tool, pack_summary, page_title,
    plural, slot_page, slot_title,
};
use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::prefs;

impl SettingsView {
    pub(super) fn modules_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(snapshot) = self.snapshot() else {
            return Vec::new();
        };
        let mut parts: Vec<AnyElement> = snapshot
            .plugins
            .iter()
            .map(|component| self.component_card(component, cx))
            .collect();
        if parts.is_empty() {
            parts.push(
                card(
                    [note("В профиле нет процессных модулей.", cx).into_any_element()],
                    cx,
                )
                .into_any_element(),
            );
        }
        parts
    }

    fn component_card(&self, plugin: &ConfigBuilderPlugin, cx: &Context<Self>) -> AnyElement {
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
                "реализация слота",
                "реализации слотов",
                "реализаций слотов",
            ));
        }
        let header = h_flex()
            .flex_wrap()
            .px_4()
            .py_3()
            .gap_x_3()
            .gap_y_1()
            .items_center()
            .child(
                h_flex()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .child(
                        Icon::new(Lucide::Boxes)
                            .small()
                            .flex_none()
                            .text_color(theme.muted_foreground),
                    )
                    .child(div().min_w_0().text_base().font_semibold().child(described(
                        hashed_id("component", &plugin.id),
                        plugin.id.clone().into(),
                        plugin.description.clone().map(Into::into),
                        Some(plugin.command.clone().into()),
                        cx,
                    ))),
            )
            .children(counts.into_iter().map(|text| chip(text, Tone::Muted, cx)));

        let mut rows = vec![self.spotted(&super::spot::component(&plugin.id), header, cx)];
        if plugin.exports.is_empty() && plugin.tool_packs.is_empty() {
            rows.push(note("Модуль не объявил exports.", cx).into_any_element());
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
            rows.push(group_label("Реализации слотов", cx));
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
        let snapshot = self.snapshot().expect("modules page needs a snapshot");
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
        v_flex()
            .child(self.spotted(&key, header, cx))
            .children(content)
            .into_any_element()
    }

    fn pack_tool_row(&self, name: &str, cx: &Context<Self>) -> AnyElement {
        let snapshot = self.snapshot().expect("modules page needs a snapshot");
        let tool = find_tool(snapshot, name);
        let runtime = tool.is_some_and(|tool| tool.runtime_managed);
        let mut chips = Vec::new();
        if let Some(tool) = tool {
            chips.push(chip(tool.safety.clone(), Tone::Muted, cx));
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
                named(
                    hashed_id("pack-tool-name", &name),
                    name.to_string(),
                    tool.map(|tool| tool.description.clone().into()),
                    None,
                    cx,
                )
                .text_sm()
                .flex_1(),
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
        let chips = [match position {
            Some(ix) => chip(format!("№ {} в цепочке", ix + 1), Tone::Success, cx),
            None => chip("выключен", Tone::Muted, cx),
        }];
        let order = position.is_some().then(|| {
            kit::button(hashed_id("hook-order", &export.id))
                .label("Порядок")
                .ghost()
                .xsmall()
                .on_click(
                    cx.listener(|this, _, window, cx| this.select_page(PageId::Hooks, window, cx)),
                )
                .into_any_element()
        });
        let switch = kit::switch(hashed_id("hook-switch", &export.id))
            .checked(position.is_some())
            .on_click(cx.listener(move |this, _: &bool, _, cx| {
                this.update_draft(cx, |draft| draft.set_hook(&id, on))
            }));
        let trailing = chips
            .into_iter()
            .map(IntoElement::into_any_element)
            .chain(order)
            .chain([switch.into_any_element()]);
        v_flex()
            .child(export_row(export, trailing, cx))
            .child(self.params_disclosure(HOOK_SLOT, &export.id, &key, open, cx))
            .into_any_element()
    }

    fn module_export_row(
        &self,
        export: &ConfigBuilderPluginExport,
        cx: &Context<Self>,
    ) -> AnyElement {
        let snapshot = self.snapshot().expect("modules page needs a snapshot");
        let title = slot_title(snapshot, &export.slot);
        let mut trailing = vec![chip(title.clone(), Tone::Muted, cx).into_any_element()];
        if export_selected(snapshot, &self.draft, export) {
            trailing.push(chip("выбран", Tone::Success, cx).into_any_element());
        }
        let page = slot_page(&export.slot);
        trailing.push(
            kit::button(hashed_id("open-slot", &(&export.slot, &export.id)))
                .label(format!("Открыть {}", page_title(Some(snapshot), &page)))
                .ghost()
                .xsmall()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_page(page.clone(), window, cx)
                }))
                .into_any_element(),
        );
        export_row(export, trailing, cx).into_any_element()
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

fn export_row(
    export: &ConfigBuilderPluginExport,
    trailing: impl IntoIterator<Item = AnyElement>,
    cx: &App,
) -> Div {
    item_row(
        named(
            hashed_id("export", &(&export.slot, &export.id)),
            export.id.clone(),
            export.description.clone().map(Into::into),
            Some(format!("{}/{}", export.slot, export.id).into()),
            cx,
        ),
        trailing,
    )
}
