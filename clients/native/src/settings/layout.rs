use proteus_contracts::app_protocol::config_builder::{ConfigBuilderSlot, ConfigBuilderSnapshot};
use proteus_contracts::domain::{ConfigField, ModuleConfigSchema};

use super::draft::{Draft, FieldPath};

const HOOK_SLOT: &str = "hook";
const MODEL_SLOT: &str = "model";
const TOOL_SLOT: &str = "tool";

/// Headless description of the settings screen; the UI only renders it.
#[derive(Clone, Debug)]
pub struct Page {
    pub id: String,
    pub icon: PageIcon,
    pub title: String,
    pub description: Option<String>,
    pub groups: Vec<Group>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageIcon {
    Model,
    Access,
    Context,
    Tools,
    Workflow,
    Hooks,
    Other,
}

/// One card on a page. `advanced` items stay folded until the user opens them.
#[derive(Clone, Debug, Default)]
pub struct Group {
    pub key: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub items: Vec<Item>,
    pub advanced: Vec<Item>,
}

#[derive(Clone, Debug)]
pub enum Item {
    Provider(Vec<Choice>),
    Mode(Vec<String>),
    Module {
        slot: String,
        options: Vec<Choice>,
        description: Option<String>,
    },
    Hook {
        id: String,
        description: Option<String>,
    },
    Tool {
        name: String,
        description: String,
        locked: bool,
    },
    Field {
        path: FieldPath,
        field: ConfigField,
    },
    Note(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

struct PageSpec {
    id: &'static str,
    icon: PageIcon,
    title: &'static str,
    description: &'static str,
}

const PAGES: &[PageSpec] = &[
    PageSpec {
        id: "model",
        icon: PageIcon::Model,
        title: "Модель",
        description: "Провайдер и параметры модели.",
    },
    PageSpec {
        id: "access",
        icon: PageIcon::Access,
        title: "Доступ",
        description: "Режим запуска и правила подтверждений.",
    },
    PageSpec {
        id: "context",
        icon: PageIcon::Context,
        title: "Контекст",
        description: "Что видит модель и как сжимается история.",
    },
    PageSpec {
        id: "tools",
        icon: PageIcon::Tools,
        title: "Инструменты",
        description: "Инструменты, доступные модели в новых запросах.",
    },
    PageSpec {
        id: "workflow",
        icon: PageIcon::Workflow,
        title: "Сценарий",
        description: "Как агент проводит ход.",
    },
    PageSpec {
        id: "hooks",
        icon: PageIcon::Hooks,
        title: "Хуки",
        description: "Обработчики, встроенные в цикл агента.",
    },
];

/// Client presentation of known slots: the page they belong to and a
/// readable section name. Unknown slots get a page of their own.
struct SectionSpec {
    slot: &'static str,
    page: &'static str,
    title: &'static str,
    description: &'static str,
}

const SECTIONS: &[SectionSpec] = &[
    SectionSpec {
        slot: "policy",
        page: "access",
        title: "Политика",
        description: "Какие действия агент выполняет сам, а какие — после подтверждения.",
    },
    SectionSpec {
        slot: "context",
        page: "context",
        title: "Сборка контекста",
        description: "Инструкции, структура проекта и найденные файлы в запросе модели.",
    },
    SectionSpec {
        slot: "compactor",
        page: "context",
        title: "Сжатие истории",
        description: "Как сокращается история, когда контекст подходит к пределу.",
    },
    SectionSpec {
        slot: "tool_exposure",
        page: "tools",
        title: "Отбор инструментов",
        description: "Какие инструменты показываются модели в каждом запросе.",
    },
    SectionSpec {
        slot: "workflow",
        page: "workflow",
        title: "Оркестратор",
        description: "Цикл хода: запросы к модели, вызовы инструментов и повторы.",
    },
];

pub fn pages(snapshot: &ConfigBuilderSnapshot, draft: &Draft) -> Vec<Page> {
    let mut pages: Vec<Page> = PAGES
        .iter()
        .map(|spec| Page {
            id: spec.id.into(),
            icon: spec.icon,
            title: spec.title.into(),
            description: Some(spec.description.into()),
            groups: Vec::new(),
        })
        .collect();
    let page = |pages: &mut Vec<Page>, id: &str| pages.iter().position(|page| page.id == id);

    let model = page(&mut pages, "model").expect("model page");
    pages[model].groups.extend(model_groups(snapshot, draft));
    if !snapshot.permission_modes.is_empty() {
        let access = page(&mut pages, "access").expect("access page");
        pages[access].groups.push(Group {
            key: "mode".into(),
            title: Some("Режим".into()),
            items: vec![Item::Mode(snapshot.permission_modes.clone())],
            ..Group::default()
        });
    }

    let mut slots: Vec<_> = snapshot.slots.iter().collect();
    slots.sort_by_key(|slot| slot.order);
    for slot in slots {
        let spec = SECTIONS.iter().find(|spec| spec.slot == slot.id);
        let index = match spec {
            Some(spec) => page(&mut pages, spec.page).expect("known page"),
            None => {
                pages.push(Page {
                    id: slot.id.clone(),
                    icon: PageIcon::Other,
                    title: slot.title.clone(),
                    description: Some(slot.responsibility.clone()).filter(|text| !text.is_empty()),
                    groups: Vec::new(),
                });
                pages.len() - 1
            }
        };
        pages[index].groups.push(slot_group(slot, spec, draft));
    }

    let tools = page(&mut pages, "tools").expect("tools page");
    pages[tools].groups.extend(tool_groups(snapshot, draft));
    let hooks = page(&mut pages, "hooks").expect("hooks page");
    pages[hooks].groups.extend(hook_groups(snapshot, draft));

    pages.retain(|page| !page.groups.is_empty());
    pages
}

/// Groups from every page whose items match `query`, folded items included.
pub fn search(pages: &[Page], query: &str) -> Vec<Group> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for page in pages {
        for group in &page.groups {
            let items: Vec<_> = group
                .items
                .iter()
                .chain(&group.advanced)
                .filter(|item| item.search_text().contains(&query))
                .cloned()
                .collect();
            if items.is_empty() {
                continue;
            }
            let title = match &group.title {
                Some(title) => format!("{} · {title}", page.title),
                None => page.title.clone(),
            };
            found.push(Group {
                key: format!("search:{}:{}", page.id, group.key),
                title: Some(title),
                description: None,
                items,
                advanced: Vec::new(),
            });
        }
    }
    found
}

impl Page {
    pub fn changed(&self, draft: &Draft, baseline: &Draft) -> bool {
        self.groups
            .iter()
            .any(|group| group.changed(draft, baseline))
    }
}

impl Group {
    pub fn changed(&self, draft: &Draft, baseline: &Draft) -> bool {
        self.items
            .iter()
            .chain(&self.advanced)
            .any(|item| item.changed(draft, baseline))
    }
}

impl Item {
    /// Whether the draft differs from the saved profile for this item.
    pub fn changed(&self, draft: &Draft, baseline: &Draft) -> bool {
        match self {
            Item::Provider(_) => draft.provider != baseline.provider,
            Item::Mode(_) => draft.mode != baseline.mode,
            Item::Module { slot, .. } => draft.modules.get(slot) != baseline.modules.get(slot),
            Item::Hook { id, .. } => draft.hooks.contains(id) != baseline.hooks.contains(id),
            Item::Tool { name, .. } => draft.tools.contains(name) != baseline.tools.contains(name),
            Item::Field { path, .. } => draft.value(path) != baseline.value(path),
            Item::Note(_) => false,
        }
    }

    fn search_text(&self) -> String {
        let text = match self {
            Item::Provider(choices) => {
                let labels: Vec<_> = choices.iter().map(|choice| choice.label.as_str()).collect();
                format!("провайдер модель provider {}", labels.join(" "))
            }
            Item::Mode(_) => "режим доступа permission mode".into(),
            Item::Module { slot, options, .. } => {
                let ids: Vec<_> = options.iter().map(|choice| choice.value.as_str()).collect();
                format!("модуль реализация {slot} {}", ids.join(" "))
            }
            Item::Hook { id, description } => {
                format!("{id} {}", description.as_deref().unwrap_or_default())
            }
            Item::Tool {
                name, description, ..
            } => format!("{name} {description}"),
            Item::Field { field, .. } => {
                format!("{} {} {}", field.title, field.key, field.description)
            }
            Item::Note(_) => String::new(),
        };
        text.to_lowercase()
    }
}

fn model_groups(snapshot: &ConfigBuilderSnapshot, draft: &Draft) -> Vec<Group> {
    let mut groups = Vec::new();
    if !snapshot.providers.is_empty() {
        groups.push(Group {
            key: "provider".into(),
            items: vec![Item::Provider(
                snapshot
                    .providers
                    .iter()
                    .map(|provider| Choice {
                        value: provider.id.clone(),
                        label: provider.label.clone(),
                    })
                    .collect(),
            )],
            ..Group::default()
        });
    }
    let model_id = draft
        .provider
        .as_ref()
        .and_then(|id| {
            snapshot
                .providers
                .iter()
                .find(|provider| &provider.id == id)
        })
        .map(|provider| provider.provider.clone())
        .or_else(|| {
            snapshot
                .model_modules
                .iter()
                .find(|module| module.active)
                .map(|module| module.id.clone())
        });
    if let Some(module) = model_id
        .as_ref()
        .and_then(|id| snapshot.model_modules.iter().find(|m| &m.id == id))
    {
        let (items, advanced) = fields(MODEL_SLOT, &module.id, module.config_schema.as_ref());
        if !items.is_empty() || !advanced.is_empty() {
            groups.push(Group {
                key: format!("model:{}", module.id),
                title: Some("Параметры модели".into()),
                description: Some(module.id.clone()),
                items,
                advanced,
            });
        }
    }
    groups
}

fn slot_group(slot: &ConfigBuilderSlot, spec: Option<&SectionSpec>, draft: &Draft) -> Group {
    let selected = draft
        .modules
        .get(&slot.id)
        .or(slot.active_module.as_ref())
        .and_then(|id| slot.modules.iter().find(|module| &module.id == id));
    let mut items = Vec::new();
    let mut advanced = Vec::new();
    if slot.modules.is_empty() {
        items.push(Item::Note("Нет доступных модулей.".into()));
    } else {
        items.push(Item::Module {
            slot: slot.id.clone(),
            options: slot
                .modules
                .iter()
                .map(|module| Choice {
                    value: module.id.clone(),
                    label: module.id.clone(),
                })
                .collect(),
            description: selected.and_then(|module| module.description.clone()),
        });
    }
    if let Some(module) = selected {
        let (basic, folded) = fields(&slot.id, &module.id, module.config_schema.as_ref());
        items.extend(basic);
        advanced = folded;
    }
    let (title, description) = match spec {
        Some(spec) => (spec.title.to_string(), Some(spec.description.to_string())),
        None => (slot.title.clone(), None),
    };
    Group {
        key: format!("slot:{}", slot.id),
        title: Some(title),
        description,
        items,
        advanced,
    }
}

fn hook_groups(snapshot: &ConfigBuilderSnapshot, draft: &Draft) -> Vec<Group> {
    if snapshot.hook_modules.is_empty() {
        return Vec::new();
    }
    let mut groups = vec![Group {
        key: "hooks".into(),
        title: Some("Цепочка".into()),
        description: Some("Хуки вызываются в порядке включения.".into()),
        items: snapshot
            .hook_modules
            .iter()
            .map(|module| Item::Hook {
                id: module.id.clone(),
                description: module.description.clone(),
            })
            .collect(),
        advanced: Vec::new(),
    }];
    for id in &draft.hooks {
        if let Some(module) = snapshot.hook_modules.iter().find(|m| &m.id == id) {
            let (items, advanced) = fields(HOOK_SLOT, &module.id, module.config_schema.as_ref());
            if !items.is_empty() || !advanced.is_empty() {
                groups.push(Group {
                    key: format!("hook:{}", module.id),
                    title: Some(module.id.clone()),
                    description: module.description.clone(),
                    items,
                    advanced,
                });
            }
        }
    }
    groups
}

fn tool_groups(snapshot: &ConfigBuilderSnapshot, draft: &Draft) -> Vec<Group> {
    let mut groups: Vec<(String, Group)> = Vec::new();
    for tool in &snapshot.tools {
        let (key, title) = if tool.runtime_managed {
            ("runtime".to_string(), "Управление агентами".to_string())
        } else if let Some(owner) = &tool.owner {
            (owner.module_id.clone(), owner.module_id.clone())
        } else {
            (tool.source.clone(), tool.source.clone())
        };
        let index = match groups.iter().position(|(existing, _)| existing == &key) {
            Some(index) => index,
            None => {
                groups.push((
                    key.clone(),
                    Group {
                        key: format!("tools:{key}"),
                        title: Some(title),
                        ..Group::default()
                    },
                ));
                groups.len() - 1
            }
        };
        groups[index].1.items.push(Item::Tool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            locked: tool.runtime_managed || (!tool.registered && !draft.tools.contains(&tool.name)),
        });
    }
    let exports = snapshot
        .plugins
        .iter()
        .flat_map(|plugin| &plugin.exports)
        .filter(|export| export.slot == TOOL_SLOT && export.active);
    for export in exports {
        let (items, advanced) = fields(TOOL_SLOT, &export.id, export.config_schema.as_ref());
        match groups.iter_mut().find(|(key, _)| key == &export.id) {
            Some((_, group)) => {
                group.description = export.description.clone();
                group.items.extend(items);
                group.advanced.extend(advanced);
            }
            None if !items.is_empty() || !advanced.is_empty() => groups.push((
                export.id.clone(),
                Group {
                    key: format!("tools:{}", export.id),
                    title: Some(export.id.clone()),
                    description: export.description.clone(),
                    items,
                    advanced,
                },
            )),
            None => {}
        }
    }
    groups.into_iter().map(|(_, group)| group).collect()
}

/// Basic and advanced field items of one module.
fn fields(slot: &str, module: &str, schema: Option<&ModuleConfigSchema>) -> (Vec<Item>, Vec<Item>) {
    let Some(schema) = schema else {
        return (Vec::new(), Vec::new());
    };
    let (advanced, basic): (Vec<_>, Vec<_>) =
        schema.fields.iter().partition(|field| field.advanced);
    let item = |field: &ConfigField| Item::Field {
        path: FieldPath::new(slot, module, &field.key),
        field: field.clone(),
    };
    (
        basic.into_iter().map(item).collect(),
        advanced.into_iter().map(item).collect(),
    )
}
