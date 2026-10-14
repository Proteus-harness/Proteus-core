//! Settings search over what the screen can show: pages, parameters of the
//! chosen modules, tools, modules, skills, connections and client rows. A
//! hit names its page, the disclosures on the way and the row to point at.

use gpui_kit::*;
use proteus_contracts::app_protocol::addons::AppAddonsSnapshot;
use proteus_contracts::app_protocol::config_builder::ConfigBuilderSnapshot;
use proteus_contracts::domain::{ConfigField, ConfigValueSchema};

use super::SettingsView;
use super::client::{APPEARANCE_ROWS, CHAT_ROWS};
use super::mcp::mcp_key;
use super::nav::nav_title;
use super::spot;
use super::tools::pack_key;
use crate::settings::FieldPath;
use crate::settings::addons;
use crate::settings::catalog::{
    CONTEXT_PROVIDER_SLOT, CONTEXT_SLOT, HOOK_SLOT, MODEL_SLOT, PageId, TOOL_SLOT, agent_pages,
    exports, find_slot, model_module, module_info, page_description, page_title, selected_module,
};
use crate::shortcuts::Command;
use crate::ui::extensions;

/// More hits than fit the navigation column only scroll it.
const LIMIT: usize = 60;

pub(super) struct Hit {
    pub(super) title: String,
    pub(super) place: String,
    pub(super) page: PageId,
    /// Disclosures to unfold on the way to the row.
    pub(super) open: Vec<String>,
    /// Row to scroll to and mark.
    pub(super) spot: Option<String>,
}

impl Hit {
    fn new(title: impl Into<String>, place: impl Into<String>, page: PageId) -> Self {
        Self {
            title: title.into(),
            place: place.into(),
            page,
            open: Vec::new(),
            spot: None,
        }
    }

    fn spot(mut self, key: String) -> Self {
        self.spot = Some(key);
        self
    }

    fn open(mut self, keys: Vec<String>) -> Self {
        self.open = keys;
        self
    }
}

/// Every word of the query must occur in the title or the other texts of a
/// hit; titles that start with the query come first, then titles that hold
/// it, then the rest in screen order.
struct Finder {
    terms: Vec<String>,
    hits: Vec<(u8, Hit)>,
}

impl Finder {
    fn new(query: &str) -> Self {
        Self {
            terms: query.split_whitespace().map(str::to_lowercase).collect(),
            hits: Vec::new(),
        }
    }

    fn add(&mut self, hit: Hit, texts: &[&str]) {
        let title = hit.title.to_lowercase();
        let rest = texts.join("\n").to_lowercase();
        if !self
            .terms
            .iter()
            .all(|term| title.contains(term.as_str()) || rest.contains(term.as_str()))
        {
            return;
        }
        let in_title = self.terms.iter().all(|term| title.contains(term.as_str()));
        let rank = match (in_title, title.starts_with(self.terms[0].as_str())) {
            (true, true) => 0,
            (true, false) => 1,
            _ => 2,
        };
        self.hits.push((rank, hit));
    }

    fn finish(mut self) -> Vec<Hit> {
        self.hits.sort_by_key(|(rank, _)| *rank);
        self.hits
            .into_iter()
            .take(LIMIT)
            .map(|(_, hit)| hit)
            .collect()
    }
}

/// Russian names of the pages titled in English.
fn page_words(page: &PageId) -> &'static str {
    match page {
        PageId::Model => "модель провайдер",
        PageId::Tools => "инструменты",
        PageId::Hooks => "обработчики хуки",
        PageId::Modules => "модули компоненты плагины",
        PageId::Mcp => "серверы",
        PageId::Skills => "навыки",
        PageId::Packages => "пакеты плагины",
        PageId::History => "история изменения",
        _ => "",
    }
}

impl SettingsView {
    pub(super) fn search(&self, query: &str, cx: &App) -> Vec<Hit> {
        let mut finder = Finder::new(query);
        if finder.terms.is_empty() {
            return Vec::new();
        }
        let snapshot = self.snapshot();
        for page in self.page_list(cx) {
            let about = match &page {
                PageId::Extension(id) => extensions::find(id)
                    .map(|manifest| manifest.description.to_string())
                    .unwrap_or_default(),
                page => page_description(snapshot, page),
            };
            finder.add(
                Hit::new(nav_title(snapshot, &page), "Раздел", page.clone()),
                &[&about, page_words(&page)],
            );
        }
        if let Some(snapshot) = snapshot {
            self.search_profile(snapshot, &mut finder);
        }
        if let Some(addons) = self.addons() {
            self.search_addons(addons, &mut finder);
        }
        for (page, rows) in [
            (PageId::Appearance, APPEARANCE_ROWS.as_slice()),
            (PageId::Chat, CHAT_ROWS.as_slice()),
        ] {
            let place = page_title(None, &page);
            for row in rows {
                finder.add(
                    Hit::new(row.title, place.clone(), page.clone()).spot(spot::row(row.title)),
                    &[row.about.unwrap_or_default()],
                );
            }
        }
        let place = page_title(None, &PageId::Shortcuts);
        for command in Command::ALL {
            finder.add(
                Hit::new(command.title(), place.clone(), PageId::Shortcuts)
                    .spot(spot::row(command.title())),
                &["сочетание клавиш"],
            );
        }
        finder.finish()
    }

    fn search_profile(&self, snapshot: &ConfigBuilderSnapshot, finder: &mut Finder) {
        for provider in &snapshot.providers {
            let title = if provider.label.is_empty() {
                &provider.model
            } else {
                &provider.label
            };
            finder.add(
                Hit::new(title, "Model · провайдер", PageId::Model)
                    .spot(spot::provider(&provider.id)),
                &[&provider.model, &provider.id, &provider.provider],
            );
        }
        if let Some(module) = model_module(snapshot, &self.draft) {
            let place = format!("Model · {module}");
            self.search_params(
                snapshot,
                finder,
                MODEL_SLOT,
                &module,
                &PageId::Model,
                &place,
                &[],
            );
        }
        for page in agent_pages(snapshot) {
            let PageId::Slot(slot) = &page else {
                continue;
            };
            let name = page_title(Some(snapshot), &page);
            for module in find_slot(snapshot, slot)
                .map(|info| info.modules.as_slice())
                .unwrap_or_default()
            {
                finder.add(
                    Hit::new(&module.id, format!("{name} · реализация"), page.clone())
                        .spot(spot::module(slot, &module.id)),
                    &[module.description.as_deref().unwrap_or_default()],
                );
            }
            if let Some(module) = selected_module(snapshot, &self.draft, slot) {
                let place = format!("{name} · {module}");
                self.search_params(snapshot, finder, slot, module, &page, &place, &[]);
            }
            if slot != CONTEXT_SLOT {
                continue;
            }
            for (plugin, export) in exports(snapshot, CONTEXT_PROVIDER_SLOT) {
                finder.add(
                    Hit::new(
                        &export.id,
                        format!("{name} · поставщик контекста"),
                        page.clone(),
                    )
                    .spot(spot::export(CONTEXT_PROVIDER_SLOT, &export.id)),
                    &[
                        export.description.as_deref().unwrap_or_default(),
                        &plugin.id,
                    ],
                );
                let open = [format!("params:{CONTEXT_PROVIDER_SLOT}:{}", export.id)];
                let place = format!("{name} · {}", export.id);
                self.search_params(
                    snapshot,
                    finder,
                    CONTEXT_PROVIDER_SLOT,
                    &export.id,
                    &page,
                    &place,
                    &open,
                );
            }
        }
        let tools = page_title(None, &PageId::Tools);
        for tool in &snapshot.tools {
            finder.add(
                Hit::new(&tool.name, tools.clone(), PageId::Tools).spot(spot::tool(&tool.name)),
                &[&tool.description, &tool.source],
            );
        }
        for module in &snapshot.hook_modules {
            finder.add(
                Hit::new(&module.id, "Hooks", PageId::Hooks).spot(spot::hook(&module.id)),
                &[module.description.as_deref().unwrap_or_default()],
            );
            // Parameters of a hook outside the chain live on its module card.
            let page = if self.draft.hooks.contains(&module.id) {
                PageId::Hooks
            } else {
                PageId::Modules
            };
            let place = format!("{} · {}", page_title(None, &page), module.id);
            let open = [format!("params:{HOOK_SLOT}:{}", module.id)];
            self.search_params(
                snapshot, finder, HOOK_SLOT, &module.id, &page, &place, &open,
            );
        }
        for plugin in &snapshot.plugins {
            finder.add(
                Hit::new(&plugin.id, "Modules · модуль", PageId::Modules)
                    .spot(spot::component(&plugin.id)),
                &[
                    plugin.description.as_deref().unwrap_or_default(),
                    &plugin.command,
                ],
            );
            for pack in &plugin.tool_packs {
                let key = pack_key(&plugin.id, &pack.id);
                let about = plugin
                    .exports
                    .iter()
                    .find(|export| export.slot == TOOL_SLOT && export.id == pack.id)
                    .and_then(|export| export.description.as_deref())
                    .unwrap_or_default();
                finder.add(
                    Hit::new(
                        &pack.id,
                        format!("Modules · {} · пакет инструментов", plugin.id),
                        PageId::Modules,
                    )
                    .spot(key.clone()),
                    &[about],
                );
                let place = format!("Modules · {}", pack.id);
                let open = [key, format!("params:{TOOL_SLOT}:{}", pack.id)];
                self.search_params(
                    snapshot,
                    finder,
                    TOOL_SLOT,
                    &pack.id,
                    &PageId::Modules,
                    &place,
                    &open,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn search_params(
        &self,
        snapshot: &ConfigBuilderSnapshot,
        finder: &mut Finder,
        slot: &str,
        module: &str,
        page: &PageId,
        place: &str,
        open: &[String],
    ) {
        let Some(schema) = module_info(snapshot, slot, module).and_then(|info| info.schema) else {
            return;
        };
        let scope = Scope { slot, module, page };
        scope.fields(finder, &schema.fields, None, place, open);
    }

    fn search_addons(&self, addons: &AppAddonsSnapshot, finder: &mut Finder) {
        let skills = page_title(None, &PageId::Skills);
        for (provider, skill) in addons::skills(addons) {
            finder.add(
                Hit::new(&skill.id, skills.clone(), PageId::Skills).spot(spot::skill(&skill.id)),
                &[&skill.name, &skill.description, provider],
            );
        }
        let mcp = page_title(None, &PageId::Mcp);
        for server in addons::mcp_servers(&self.draft, Some(addons)) {
            let command = server
                .configured
                .map(|server| server.command.as_str())
                .unwrap_or_default();
            finder.add(
                Hit::new(server.name, mcp.clone(), PageId::Mcp).open(vec![mcp_key(server.name)]),
                &[command, server.package.unwrap_or_default()],
            );
        }
        let packages = page_title(None, &PageId::Packages);
        for package in &addons.plugins {
            let path = package.path.to_string_lossy();
            let title = package.name.clone().unwrap_or_else(|| path.to_string());
            finder.add(
                Hit::new(title, packages.clone(), PageId::Packages),
                &[package.description.as_deref().unwrap_or_default(), &path],
            );
        }
    }
}

/// Parameters of one module on one page.
struct Scope<'a> {
    slot: &'a str,
    module: &'a str,
    page: &'a PageId,
}

impl Scope<'_> {
    /// Fields of one object level; advanced ones and nested groups add their
    /// disclosures to the way.
    fn fields(
        &self,
        finder: &mut Finder,
        fields: &[ConfigField],
        parent: Option<&FieldPath>,
        place: &str,
        open: &[String],
    ) {
        let scope = match parent {
            None => format!("{}:{}", self.slot, self.module),
            Some(path) => format!("{}:{}:{}", self.slot, self.module, path.dotted()),
        };
        for field in fields {
            let path = match parent {
                None => FieldPath::new(self.slot, self.module, &field.key),
                Some(parent) => parent.child(&field.key),
            };
            let mut way = open.to_vec();
            if field.advanced {
                way.push(format!("advanced:{scope}"));
            }
            finder.add(
                Hit::new(&field.title, place, self.page.clone())
                    .open(way.clone())
                    .spot(spot::field(&path)),
                &[&field.key, &field.description],
            );
            if let ConfigValueSchema::Object { fields: nested } = &field.value
                && !nested.is_empty()
            {
                way.push(format!(
                    "group:{}:{}:{}",
                    self.slot,
                    self.module,
                    path.dotted()
                ));
                let place = format!("{place} · {}", field.title);
                self.fields(finder, nested, Some(&path), &place, &way);
            }
        }
    }
}
