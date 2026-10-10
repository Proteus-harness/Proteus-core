use proteus_contracts::app_protocol::config_builder::ConfigBuilderSnapshot;
use proteus_contracts::domain::{ConfigField, ModuleConfigSchema};

use super::draft::{Draft, FieldPath};

const HOOK_SLOT: &str = "hook";
const MODEL_SLOT: &str = "model";
const TOOL_SLOT: &str = "tool";

/// Headless description of the settings screen; the UI only renders it.
#[derive(Clone, Debug)]
pub struct Page {
    pub title: String,
    pub description: Option<String>,
    pub groups: Vec<Group>,
}

#[derive(Clone, Debug, Default)]
pub struct Group {
    pub title: Option<String>,
    pub description: Option<String>,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug)]
pub enum Item {
    Provider(Vec<Choice>),
    Mode(Vec<String>),
    Module {
        slot: String,
        options: Vec<Choice>,
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

pub fn pages(snapshot: &ConfigBuilderSnapshot, draft: &Draft) -> Vec<Page> {
    let mut pages = vec![general_page(snapshot, draft)];
    let mut slots: Vec<_> = snapshot.slots.iter().collect();
    slots.sort_by_key(|slot| slot.order);
    for slot in slots {
        let selected = draft.modules.get(&slot.id).or(slot.active_module.as_ref());
        let mut groups = vec![Group {
            title: Some("Модуль".into()),
            description: None,
            items: if slot.modules.is_empty() {
                vec![Item::Note("Для этого слота нет доступных модулей.".into())]
            } else {
                vec![Item::Module {
                    slot: slot.id.clone(),
                    options: slot
                        .modules
                        .iter()
                        .map(|module| Choice {
                            value: module.id.clone(),
                            label: module.id.clone(),
                        })
                        .collect(),
                }]
            },
        }];
        if let Some(module) = selected.and_then(|id| slot.modules.iter().find(|m| &m.id == id)) {
            let fields = field_groups(&slot.id, &module.id, module.config_schema.as_ref(), None);
            if fields.is_empty() {
                groups[0]
                    .items
                    .push(Item::Note("У модуля нет настраиваемых параметров.".into()));
            }
            groups.extend(fields);
        }
        pages.push(Page {
            title: slot.title.clone(),
            description: Some(slot.responsibility.clone()).filter(|text| !text.is_empty()),
            groups,
        });
    }
    if !snapshot.hook_modules.is_empty() {
        pages.push(hooks_page(snapshot, draft));
    }
    if !snapshot.tools.is_empty() {
        pages.push(tools_page(snapshot, draft));
    }
    pages
}

fn general_page(snapshot: &ConfigBuilderSnapshot, draft: &Draft) -> Page {
    let mut build = Group {
        title: Some("Сборка".into()),
        description: None,
        items: Vec::new(),
    };
    if !snapshot.providers.is_empty() {
        build.items.push(Item::Provider(
            snapshot
                .providers
                .iter()
                .map(|provider| Choice {
                    value: provider.id.clone(),
                    label: provider.label.clone(),
                })
                .collect(),
        ));
    }
    if !snapshot.permission_modes.is_empty() {
        build
            .items
            .push(Item::Mode(snapshot.permission_modes.clone()));
    }
    let mut groups = vec![build];
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
        groups.extend(field_groups(
            MODEL_SLOT,
            &module.id,
            module.config_schema.as_ref(),
            Some(&format!("Модель · {}", module.id)),
        ));
    }
    Page {
        title: "Общие".into(),
        description: Some("Провайдер, режим доступа и параметры модели.".into()),
        groups,
    }
}

fn hooks_page(snapshot: &ConfigBuilderSnapshot, draft: &Draft) -> Page {
    let mut groups = vec![Group {
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
    }];
    for id in &draft.hooks {
        if let Some(module) = snapshot.hook_modules.iter().find(|m| &m.id == id) {
            groups.extend(field_groups(
                HOOK_SLOT,
                &module.id,
                module.config_schema.as_ref(),
                Some(&module.id),
            ));
        }
    }
    Page {
        title: "Хуки".into(),
        description: None,
        groups,
    }
}

fn tools_page(snapshot: &ConfigBuilderSnapshot, draft: &Draft) -> Page {
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
                    key,
                    Group {
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
        let fields = field_groups(TOOL_SLOT, &export.id, export.config_schema.as_ref(), None);
        let items = fields.into_iter().flat_map(|group| group.items);
        match groups.iter_mut().find(|(key, _)| key == &export.id) {
            Some((_, group)) => {
                group.description = export.description.clone();
                group.items.extend(items);
            }
            None => {
                let items: Vec<_> = items.collect();
                if !items.is_empty() {
                    groups.push((
                        export.id.clone(),
                        Group {
                            title: Some(export.id.clone()),
                            description: export.description.clone(),
                            items,
                        },
                    ));
                }
            }
        }
    }
    Page {
        title: "Инструменты".into(),
        description: Some("Включённые инструменты доступны модели в новых запросах.".into()),
        groups: groups.into_iter().map(|(_, group)| group).collect(),
    }
}

/// Basic and advanced field groups of one module. `title` names the basic
/// group on pages that hold several modules.
fn field_groups(
    slot: &str,
    module: &str,
    schema: Option<&ModuleConfigSchema>,
    title: Option<&str>,
) -> Vec<Group> {
    let Some(schema) = schema else {
        return Vec::new();
    };
    let item = |field: &ConfigField| Item::Field {
        path: FieldPath::new(slot, module, &field.key),
        field: field.clone(),
    };
    let basic: Vec<_> = schema
        .fields
        .iter()
        .filter(|field| !field.advanced)
        .map(item)
        .collect();
    let advanced: Vec<_> = schema
        .fields
        .iter()
        .filter(|field| field.advanced)
        .map(item)
        .collect();
    let mut groups = Vec::new();
    if !basic.is_empty() {
        groups.push(Group {
            title: Some(title.unwrap_or("Параметры").to_string()),
            description: None,
            items: basic,
        });
    }
    if !advanced.is_empty() {
        groups.push(Group {
            title: Some(match title {
                Some(title) => format!("{title} · дополнительно"),
                None => "Дополнительно".to_string(),
            }),
            description: None,
            items: advanced,
        });
    }
    groups
}
