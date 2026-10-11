//! Presentation of the builder snapshot: pages, labels and lookups shared by
//! the settings screens. Slot ids are contract-level names, so mapping them to
//! pages and titles here does not depend on any concrete module.

use proteus_contracts::app_protocol::config_builder::{
    ConfigBuilderPlugin, ConfigBuilderPluginExport, ConfigBuilderSlot, ConfigBuilderSnapshot,
    ConfigBuilderTool, ConfigBuilderToolPack,
};
use proteus_contracts::domain::ModuleConfigSchema;

use super::draft::{Area, Draft};

pub const MODEL_SLOT: &str = "model";
pub const HOOK_SLOT: &str = "hook";
pub const TOOL_SLOT: &str = "tool";
pub const CONTEXT_PROVIDER_SLOT: &str = "context_provider";
pub const POLICY_SLOT: &str = "policy";

/// Select-one slots with their own page, in navigation order.
const SLOT_PAGES: [&str; 5] = [
    "workflow",
    "context",
    "compactor",
    "tool_exposure",
    "policy",
];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PageId {
    Model,
    Slot(String),
    ContextProviders,
    Tools,
    Hooks,
    Plugins,
    History,
    Appearance,
    Chat,
    Extensions,
}

impl PageId {
    pub fn is_agent(&self) -> bool {
        !matches!(self, Self::Appearance | Self::Chat | Self::Extensions)
    }
}

/// Technical title and explanation of a known slot.
pub fn slot_text(slot: &str) -> Option<(&'static str, &'static str)> {
    Some(match slot {
        MODEL_SLOT => ("Model", "Модель профиля и параметры её модуля."),
        "workflow" => (
            "Workflow",
            "Управляет шагами агента от запроса до результата.",
        ),
        "context" => ("Context", "Собирает сведения, которые получает модель."),
        CONTEXT_PROVIDER_SLOT => (
            "Context Providers",
            "Поставляют навыки, поиск и память. Какие из них читать автоматически, задают параметры модуля Context.",
        ),
        "compactor" => (
            "Compactor",
            "Сокращает длинную историю без потери рабочего контекста.",
        ),
        "tool_exposure" => (
            "Tool Exposure",
            "Определяет, какие инструменты видит модель.",
        ),
        POLICY_SLOT => (
            "Policy",
            "Режим прав профиля и политика, которая решает, какие действия требуют подтверждения. Режим текущего чата меняется в поле ввода.",
        ),
        TOOL_SLOT => ("Tools", "Инструменты, которые профиль разрешает агенту."),
        HOOK_SLOT => (
            "Hooks",
            "Обработчики дополняют шаги агента: проверяют вызовы инструментов, добавляют инструкции и проверяют завершение. Вызываются по порядку списка.",
        ),
        _ => return None,
    })
}

pub fn slot_title(snapshot: &ConfigBuilderSnapshot, slot: &str) -> String {
    slot_text(slot)
        .map(|(title, _)| title.to_string())
        .or_else(|| find_slot(snapshot, slot).map(|slot| slot.title.clone()))
        .unwrap_or_else(|| slot.to_string())
}

/// Agent pages: the nine slots, then unknown select-one slots, plugins and
/// history.
pub fn agent_pages(snapshot: &ConfigBuilderSnapshot) -> Vec<PageId> {
    let mut pages = vec![PageId::Model];
    pages.extend(
        ["workflow", "context"]
            .into_iter()
            .map(|slot| PageId::Slot(slot.to_string())),
    );
    pages.push(PageId::ContextProviders);
    pages.push(PageId::Slot("compactor".to_string()));
    pages.push(PageId::Tools);
    pages.extend(
        ["tool_exposure", POLICY_SLOT]
            .into_iter()
            .map(|slot| PageId::Slot(slot.to_string())),
    );
    pages.push(PageId::Hooks);
    let mut unknown: Vec<&ConfigBuilderSlot> = snapshot
        .slots
        .iter()
        .filter(|slot| !SLOT_PAGES.contains(&slot.id.as_str()))
        .collect();
    unknown.sort_by_key(|slot| slot.order);
    pages.extend(
        unknown
            .into_iter()
            .map(|slot| PageId::Slot(slot.id.clone())),
    );
    pages.push(PageId::Plugins);
    pages.push(PageId::History);
    pages
}

pub const CLIENT_PAGES: [PageId; 3] = [PageId::Appearance, PageId::Chat, PageId::Extensions];

pub fn page_title(snapshot: Option<&ConfigBuilderSnapshot>, page: &PageId) -> String {
    match page {
        PageId::Model => "Model".into(),
        PageId::Slot(slot) => match snapshot {
            Some(snapshot) => slot_title(snapshot, slot),
            None => slot_text(slot)
                .map(|(title, _)| title.to_string())
                .unwrap_or_else(|| slot.clone()),
        },
        PageId::ContextProviders => "Context Providers".into(),
        PageId::Tools => "Tools".into(),
        PageId::Hooks => "Hooks".into(),
        PageId::Plugins => "Plugins".into(),
        PageId::History => "History".into(),
        PageId::Appearance => "Внешний вид".into(),
        PageId::Chat => "Чат".into(),
        PageId::Extensions => "Расширения".into(),
    }
}

pub fn page_description(snapshot: Option<&ConfigBuilderSnapshot>, page: &PageId) -> String {
    let text = match page {
        PageId::Model => {
            "Модель профиля по умолчанию. Модель и уровень рассуждения текущего чата можно сменить в поле ввода."
        }
        PageId::Slot(slot) => {
            return slot_text(slot)
                .map(|(_, text)| text.to_string())
                .or_else(|| {
                    snapshot
                        .and_then(|snapshot| find_slot(snapshot, slot))
                        .map(|slot| slot.responsibility.clone())
                })
                .unwrap_or_default();
        }
        PageId::ContextProviders => slot_text(CONTEXT_PROVIDER_SLOT)
            .map(|(_, text)| text)
            .unwrap_or(""),
        PageId::Tools => slot_text(TOOL_SLOT).map(|(_, text)| text).unwrap_or(""),
        PageId::Hooks => slot_text(HOOK_SLOT).map(|(_, text)| text).unwrap_or(""),
        PageId::Plugins => {
            "Плагины — процессные компоненты профиля. Один плагин может давать пакеты инструментов, обработчики и модули слотов."
        }
        PageId::History => {
            "Каждое сохранение настроек агента запоминает состояние профиля, которое оно заменило. «Вернуть как было» загружает это состояние в черновик: проверьте изменения внизу и сохраните. Откат тоже попадает в историю."
        }
        PageId::Appearance => "Тема и анимации интерфейса. Сохраняются на этом устройстве.",
        PageId::Chat => "Отправка сообщений. Сохраняется на этом устройстве.",
        PageId::Extensions => "Значки, панели и вкладки. Сохраняются на этом устройстве.",
    };
    text.to_string()
}

/// Page that shows a slot's implementation.
pub fn slot_page(slot: &str) -> PageId {
    match slot {
        MODEL_SLOT => PageId::Model,
        HOOK_SLOT => PageId::Hooks,
        TOOL_SLOT => PageId::Tools,
        CONTEXT_PROVIDER_SLOT => PageId::ContextProviders,
        _ => PageId::Slot(slot.to_string()),
    }
}

/// Which page an edited area belongs to.
pub fn area_page(area: &Area) -> PageId {
    match area {
        Area::Provider => PageId::Model,
        Area::Mode => PageId::Slot(POLICY_SLOT.to_string()),
        Area::Tools => PageId::Tools,
        Area::Hooks => PageId::Hooks,
        Area::Addons => PageId::Plugins,
        // Tool pack parameters are edited with their packs.
        Area::Slot(slot) if slot == TOOL_SLOT => PageId::Plugins,
        Area::Slot(slot) => slot_page(slot),
    }
}

/// Labels of changed areas for the save bar, deduplicated in order.
pub fn area_labels(snapshot: &ConfigBuilderSnapshot, areas: &[Area]) -> Vec<String> {
    let mut labels: Vec<String> = Vec::new();
    for area in areas {
        let label = match area {
            Area::Provider => "Model".to_string(),
            Area::Mode => "Режим прав".to_string(),
            Area::Tools => "Tools".to_string(),
            Area::Hooks => "Hooks".to_string(),
            Area::Addons => "Дополнения агента".to_string(),
            Area::Slot(slot) => slot_title(snapshot, slot),
        };
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels
}

pub fn safety_label(safety: &str) -> &str {
    match safety {
        "ReadOnly" => "только чтение",
        "WritesFiles" => "меняет файлы",
        "RunsCommands" => "запускает команды",
        "Network" => "сеть",
        "Dangerous" => "опасный",
        other => other,
    }
}

pub fn permission_text(mode: &str) -> (&str, &'static str) {
    match mode {
        "plan" => ("Только чтение", "Доступны только инструменты чтения."),
        "normal" => (
            "По правилам",
            "Решения принимает политика подтверждений; спорные действия ждут вашего ответа.",
        ),
        "auto" => (
            "Правки без вопросов",
            "Чтение и изменение файлов без подтверждения; команды и сеть запрещены.",
        ),
        other => (other, ""),
    }
}

pub fn find_slot<'a>(
    snapshot: &'a ConfigBuilderSnapshot,
    slot: &str,
) -> Option<&'a ConfigBuilderSlot> {
    snapshot.slots.iter().find(|candidate| candidate.id == slot)
}

/// Every export of the profile's process plugins for one slot.
pub fn exports<'a>(
    snapshot: &'a ConfigBuilderSnapshot,
    slot: &'a str,
) -> impl Iterator<Item = (&'a ConfigBuilderPlugin, &'a ConfigBuilderPluginExport)> + 'a {
    snapshot.plugins.iter().flat_map(move |plugin| {
        plugin
            .exports
            .iter()
            .filter(move |export| export.slot == slot)
            .map(move |export| (plugin, export))
    })
}

/// Module described by the snapshot: its schema and description.
pub struct ModuleInfo<'a> {
    pub schema: Option<&'a ModuleConfigSchema>,
    pub description: Option<&'a str>,
}

pub fn module_info<'a>(
    snapshot: &'a ConfigBuilderSnapshot,
    slot: &str,
    module: &str,
) -> Option<ModuleInfo<'a>> {
    let modules = match slot {
        MODEL_SLOT => Some(&snapshot.model_modules),
        HOOK_SLOT => Some(&snapshot.hook_modules),
        _ => find_slot(snapshot, slot).map(|slot| &slot.modules),
    };
    if let Some(found) = modules
        .into_iter()
        .flatten()
        .find(|candidate| candidate.id == module)
    {
        return Some(ModuleInfo {
            schema: found.config_schema.as_ref(),
            description: found.description.as_deref(),
        });
    }
    snapshot
        .plugins
        .iter()
        .flat_map(|plugin| &plugin.exports)
        .find(|export| export.slot == slot && export.id == module)
        .map(|export| ModuleInfo {
            schema: export.config_schema.as_ref(),
            description: export.description.as_deref(),
        })
}

/// Model module configured by the draft's provider.
pub fn model_module(snapshot: &ConfigBuilderSnapshot, draft: &Draft) -> Option<String> {
    draft
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
        })
}

/// Module of a select-one slot that the draft uses.
pub fn selected_module<'a>(
    snapshot: &'a ConfigBuilderSnapshot,
    draft: &'a Draft,
    slot: &str,
) -> Option<&'a str> {
    draft
        .modules
        .get(slot)
        .map(String::as_str)
        .or_else(|| find_slot(snapshot, slot)?.active_module.as_deref())
}

/// Whether an export is the one the draft uses in its slot.
pub fn export_selected(
    snapshot: &ConfigBuilderSnapshot,
    draft: &Draft,
    export: &ConfigBuilderPluginExport,
) -> bool {
    match export.slot.as_str() {
        MODEL_SLOT => model_module(snapshot, draft).as_deref() == Some(export.id.as_str()),
        HOOK_SLOT => draft.hooks.contains(&export.id),
        TOOL_SLOT | CONTEXT_PROVIDER_SLOT => export.active,
        slot => selected_module(snapshot, draft, slot) == Some(export.id.as_str()),
    }
}

pub fn find_tool<'a>(
    snapshot: &'a ConfigBuilderSnapshot,
    name: &str,
) -> Option<&'a ConfigBuilderTool> {
    snapshot.tools.iter().find(|tool| tool.name == name)
}

pub fn tool_matches(tool: &ConfigBuilderTool, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || tool.name.to_lowercase().contains(&query)
        || tool.description.to_lowercase().contains(&query)
}

/// Tool names enabled in the draft that the snapshot does not know.
pub fn missing_tools<'a>(snapshot: &ConfigBuilderSnapshot, draft: &'a Draft) -> Vec<&'a str> {
    draft
        .tools
        .iter()
        .filter(|name| find_tool(snapshot, name).is_none())
        .map(String::as_str)
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackState {
    On,
    Off,
    Mixed,
}

pub struct PackSummary {
    pub enabled: usize,
    pub total: usize,
    /// Tools a pack switch toggles: runtime-managed ones stay as they are.
    pub switchable: Vec<String>,
    pub state: PackState,
}

pub fn pack_summary(
    snapshot: &ConfigBuilderSnapshot,
    draft: &Draft,
    pack: &ConfigBuilderToolPack,
) -> PackSummary {
    let switchable: Vec<String> = pack
        .tools
        .iter()
        .filter(|name| !find_tool(snapshot, name).is_some_and(|tool| tool.runtime_managed))
        .cloned()
        .collect();
    let on = switchable
        .iter()
        .filter(|name| draft.tools.contains(*name))
        .count();
    let state = match on {
        0 => PackState::Off,
        on if on == switchable.len() => PackState::On,
        _ => PackState::Mixed,
    };
    PackSummary {
        enabled: pack
            .tools
            .iter()
            .filter(|name| {
                draft.tools.contains(*name)
                    || find_tool(snapshot, name).is_some_and(|tool| tool.runtime_managed)
            })
            .count(),
        total: pack.tools.len(),
        switchable,
        state,
    }
}

/// Russian count with the noun form for 1 / 2–4 / 5+.
pub fn plural(count: usize, one: &str, few: &str, many: &str) -> String {
    let (tens, units) = (count % 100, count % 10);
    let word = if (11..=14).contains(&tens) {
        many
    } else if units == 1 {
        one
    } else if (2..=4).contains(&units) {
        few
    } else {
        many
    };
    format!("{count} {word}")
}
