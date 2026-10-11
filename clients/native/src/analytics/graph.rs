//! Build topology presentation: slot catalog and the graph map model with
//! its column layout (ported from the web architecture page). Edges always
//! come from the server snapshot; nothing is inferred here.

use std::collections::{BTreeSet, HashSet};

use proteus_contracts::app_protocol::topology::{
    ModuleTopology, SlotTopology, TopologyEdge, TopologySnapshot,
};

pub const NODE_WIDTH: f32 = 180.;
pub const NODE_HEIGHT: f32 = 64.;
const GAP_X: f32 = 224.;
const GAP_Y: f32 = 80.;
const MARGIN_X: f32 = 32.;
const MARGIN_Y: f32 = 32.;

#[derive(Clone, Debug)]
pub struct SlotView {
    pub slot: SlotTopology,
    pub active: Option<ModuleTopology>,
    pub alternatives: Vec<ModuleTopology>,
}

pub fn slot_views(snapshot: &TopologySnapshot) -> Vec<SlotView> {
    let mut views: Vec<SlotView> = snapshot
        .slots
        .iter()
        .map(|slot| {
            let active = slot.active_module.as_ref().and_then(|active| {
                snapshot
                    .modules
                    .iter()
                    .find(|module| module.slot == slot.id && module.id == *active)
                    .cloned()
            });
            let mut alternatives: Vec<ModuleTopology> = snapshot
                .modules
                .iter()
                .filter(|module| module.slot == slot.id && !module.active)
                .cloned()
                .collect();
            alternatives.sort_by(|left, right| left.id.cmp(&right.id));
            SlotView {
                slot: slot.clone(),
                active,
                alternatives,
            }
        })
        .collect();
    views.sort_by(|left, right| {
        (left.slot.order, &left.slot.id).cmp(&(right.slot.order, &right.slot.id))
    });
    views
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Config,
    Tools,
    Slot,
    Module,
    Tool,
    Other,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Config => "ПРОФИЛЬ",
            Self::Tools => "ИНСТРУМЕНТЫ",
            Self::Slot => "СЛОТ",
            Self::Module => "МОДУЛЬ",
            Self::Tool => "ИНСТРУМЕНТ",
            Self::Other => "УЗЕЛ",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: String,
    pub kind: Kind,
    pub label: String,
    pub subtitle: String,
    pub description: String,
    pub facts: Vec<(String, String)>,
    pub category: String,
    pub slot: String,
    pub active: bool,
    pub missing: bool,
    pub schema: Option<serde_json::Value>,
}

impl Node {
    fn new(id: impl Into<String>, kind: Kind, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind,
            label: label.into(),
            subtitle: String::new(),
            description: String::new(),
            facts: Vec::new(),
            category: String::new(),
            slot: String::new(),
            active: true,
            missing: false,
            schema: None,
        }
    }

    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        format!(
            "{} {} {} {}",
            self.label, self.subtitle, self.id, self.description
        )
        .to_lowercase()
        .contains(&query)
    }
}

fn yes(value: bool) -> String {
    if value { "Да" } else { "Нет" }.into()
}

#[derive(Clone, Debug)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<TopologyEdge>,
}

impl Graph {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    pub fn related(&self, selected: &str) -> HashSet<String> {
        let mut out = HashSet::from([selected.to_string()]);
        for edge in &self.edges {
            if edge.from == selected {
                out.insert(edge.to.clone());
            }
            if edge.to == selected {
                out.insert(edge.from.clone());
            }
        }
        out
    }
}

pub fn graph(snapshot: &TopologySnapshot) -> Graph {
    let mut nodes = Vec::new();
    let mut config = Node::new(
        "config",
        Kind::Config,
        if snapshot.profile.is_empty() {
            "Профиль"
        } else {
            &snapshot.profile
        },
    );
    config.subtitle = "Конфигурация".into();
    config.description = "Текущая сборка агента".into();
    config.facts = vec![
        ("Каталог".into(), snapshot.cwd.clone()),
        (
            "Файл".into(),
            snapshot
                .config_path
                .clone()
                .unwrap_or_else(|| "Автоматический выбор".into()),
        ),
        ("Ревизия".into(), snapshot.module_epoch.to_string()),
        ("Доступ".into(), snapshot.permission_mode.clone()),
    ];
    config.facts.extend(
        snapshot
            .config_files
            .iter()
            .map(|file| ("Источник".into(), file.clone())),
    );
    nodes.push(config);

    let mut tools = Node::new("tools", Kind::Tools, "Инструменты");
    tools.subtitle = format!(
        "{} зарегистрировано",
        snapshot.tools.iter().filter(|tool| tool.registered).count()
    );
    tools.description = "Инструменты текущей сборки".into();
    nodes.push(tools);

    let mut slots: Vec<&SlotTopology> = snapshot.slots.iter().collect();
    slots.sort_by(|left, right| (left.order, &left.id).cmp(&(right.order, &right.id)));
    for slot in slots {
        let title = if slot.title.is_empty() {
            &slot.id
        } else {
            &slot.title
        };
        let mut node = Node::new(format!("slot:{}", slot.id), Kind::Slot, title.clone());
        let active = slot
            .active_module
            .clone()
            .unwrap_or_else(|| "Модуль не выбран".into());
        node.subtitle = active.clone();
        node.description = slot.responsibility.clone();
        node.facts = vec![
            ("Slot".into(), slot.id.clone()),
            ("Категория".into(), slot.category.clone()),
            ("Обязательный".into(), yes(slot.required)),
            ("Выбранный модуль".into(), active),
        ];
        node.category = slot.category.clone();
        node.slot = slot.id.clone();
        node.active = slot.active_module.is_some();
        node.missing = slot.required && slot.active_module.is_none();
        nodes.push(node);
    }
    for module in &snapshot.modules {
        let mut node = Node::new(
            format!("module:{}:{}", module.slot, module.id),
            Kind::Module,
            module.id.clone(),
        );
        node.subtitle = format!(
            "{} · {}",
            module.slot,
            if module.active {
                "выбран"
            } else {
                "доступен"
            }
        );
        node.description = module.description.clone().unwrap_or_default();
        let capabilities = if module.capabilities.is_empty() {
            "Не указаны".into()
        } else {
            module.capabilities.join(", ")
        };
        node.facts = vec![
            ("Slot".into(), module.slot.clone()),
            ("Источник".into(), module.source.kind().into()),
            ("Версия".into(), module.version.clone()),
            ("API".into(), module.api_version.clone()),
            ("Возможности".into(), capabilities),
        ];
        node.slot = module.slot.clone();
        node.active = module.active;
        nodes.push(node);
    }
    for tool in &snapshot.tools {
        let mut node = Node::new(format!("tool:{}", tool.name), Kind::Tool, tool.name.clone());
        node.subtitle = match (tool.registered, tool.enabled) {
            (false, _) => "Не зарегистрирован",
            (true, true) => "Включён",
            (true, false) => "Отключён",
        }
        .into();
        node.description = tool.description.clone();
        node.facts = vec![
            ("Источник".into(), tool.source.clone()),
            ("Safety".into(), tool.safety.clone()),
            ("Регистрация".into(), yes(tool.registered)),
            ("Включён".into(), yes(tool.enabled)),
        ];
        node.active = tool.registered && tool.enabled;
        node.schema = Some(tool.input_schema.clone());
        nodes.push(node);
    }
    for edge in &snapshot.edges {
        for id in [&edge.from, &edge.to] {
            if !nodes.iter().any(|node| node.id == *id) {
                let mut node = Node::new(id.clone(), Kind::Other, id.clone());
                node.subtitle = "Связь из snapshot".into();
                node.description = "Узел без подробного описания в текущем snapshot.".into();
                nodes.push(node);
            }
        }
    }
    Graph {
        nodes,
        edges: snapshot.edges.clone(),
    }
}

pub fn edge_label(edge: &TopologyEdge) -> String {
    if let Some(label) = &edge.label {
        return label.clone();
    }
    match edge.kind.as_str() {
        "selects" => "Выбор",
        "active_module" => "Выбран",
        "available_module" => "Доступен",
        "runtime" => "Используется",
        "registered_tool" => "Зарегистрирован",
        "unregistered_tool" => "Не зарегистрирован",
        "enables" => "Включает",
        "uses" => "Использует",
        other => other,
    }
    .into()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Assembly,
    Modules,
    Tools,
}

impl Scope {
    pub const ALL: [Self; 3] = [Self::Assembly, Self::Modules, Self::Tools];

    pub fn label(self) -> &'static str {
        match self {
            Self::Assembly => "Сборка",
            Self::Modules => "Модули",
            Self::Tools => "Инструменты",
        }
    }
}

/// Node index in `Graph::nodes` with its top-left corner in map units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub node: usize,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub nodes: Vec<Placed>,
    /// Indexes into `Graph::edges` whose ends are both placed.
    pub edges: Vec<usize>,
    pub width: f32,
    pub height: f32,
}

impl Layout {
    pub fn position(&self, graph: &Graph, id: &str) -> Option<Placed> {
        self.nodes
            .iter()
            .copied()
            .find(|placed| graph.nodes[placed.node].id == id)
    }
}

pub fn layout(graph: &Graph, scope: Scope, inactive: bool) -> Layout {
    let mut placed = Vec::new();
    let mut place = |node: usize, column: f32, row: f32| {
        placed.push(Placed {
            node,
            x: MARGIN_X + column * GAP_X,
            y: MARGIN_Y + row * GAP_Y,
        });
    };
    let of_kind = |kind: Kind| {
        graph
            .nodes
            .iter()
            .enumerate()
            .filter(move |(_, node)| node.kind == kind)
    };
    match scope {
        Scope::Modules => {
            let mut row = 0.;
            let slots: BTreeSet<&str> = of_kind(Kind::Slot)
                .map(|(_, node)| node.slot.as_str())
                .chain(of_kind(Kind::Module).map(|(_, node)| node.slot.as_str()))
                .collect();
            let mut slots: Vec<&str> = slots.into_iter().collect();
            slots.sort_by_key(|slot| {
                graph
                    .nodes
                    .iter()
                    .position(|node| node.kind == Kind::Slot && node.slot == *slot)
                    .unwrap_or(usize::MAX)
            });
            for slot in slots {
                let modules: Vec<usize> = of_kind(Kind::Module)
                    .filter(|(_, node)| node.slot == slot && (inactive || node.active))
                    .map(|(index, _)| index)
                    .collect();
                if modules.is_empty() && !inactive {
                    continue;
                }
                if let Some((index, _)) = of_kind(Kind::Slot).find(|(_, node)| node.slot == slot) {
                    place(index, 0., row);
                }
                for (offset, index) in modules.iter().enumerate() {
                    place(*index, 1. + (offset % 3) as f32, row + (offset / 3) as f32);
                }
                row += modules.len().div_ceil(3).max(1) as f32 + 0.35;
            }
        }
        Scope::Tools => {
            if let Some((index, _)) = of_kind(Kind::Tools).next() {
                place(index, 0., 0.);
            }
            let tools: Vec<usize> = of_kind(Kind::Tool)
                .filter(|(_, node)| inactive || node.active)
                .map(|(index, _)| index)
                .collect();
            for (offset, index) in tools.iter().enumerate() {
                place(*index, 1. + (offset % 3) as f32, (offset / 3) as f32);
            }
        }
        Scope::Assembly => {
            let mut columns: [Vec<usize>; 4] = Default::default();
            for (index, node) in graph.nodes.iter().enumerate() {
                let column = match node.kind {
                    Kind::Module | Kind::Tool => continue,
                    Kind::Config => 0,
                    Kind::Tools => 2,
                    _ if node.category == "orchestrator" => 0,
                    _ if matches!(node.category.as_str(), "backend" | "post_turn") => 3,
                    _ => 1,
                };
                columns[column].push(index);
            }
            let rows = columns.iter().map(Vec::len).max().unwrap_or(0);
            for (column, nodes) in columns.iter().enumerate() {
                let shift = (rows - nodes.len()) as f32 / 2.;
                for (row, index) in nodes.iter().enumerate() {
                    place(*index, column as f32, row as f32 + shift);
                }
            }
        }
    }
    let visible: HashSet<&str> = placed
        .iter()
        .map(|placed| graph.nodes[placed.node].id.as_str())
        .collect();
    let edges = graph
        .edges
        .iter()
        .enumerate()
        .filter(|(_, edge)| {
            visible.contains(edge.from.as_str()) && visible.contains(edge.to.as_str())
        })
        .map(|(index, _)| index)
        .collect();
    let width = placed
        .iter()
        .map(|placed| placed.x + NODE_WIDTH + MARGIN_X)
        .fold(600., f32::max);
    let height = placed
        .iter()
        .map(|placed| placed.y + NODE_HEIGHT + MARGIN_Y)
        .fold(300., f32::max);
    Layout {
        nodes: placed,
        edges,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot() -> TopologySnapshot {
        serde_json::from_value(json!({
            "profile": "codex", "cwd": "/w", "config_path": null, "config_files": [],
            "module_epoch": 1, "permission_mode": "default", "model": null,
            "slots": [
                {"id": "workflow", "title": "Workflow", "responsibility": "r", "active_module": "loop", "required": true, "category": "orchestrator", "order": 1},
                {"id": "context", "title": "Context", "responsibility": "r", "active_module": null, "required": true, "category": "pipeline", "order": 2},
            ],
            "hooks": [],
            "modules": [
                {"id": "loop", "slot": "workflow", "active": true, "source": {"kind": "builtin"}, "version": "1", "api_version": "1", "capabilities": [], "description": null},
                {"id": "alt", "slot": "workflow", "active": false, "source": {"kind": "builtin"}, "version": "1", "api_version": "1", "capabilities": [], "description": null},
            ],
            "tools": [],
            "edges": [
                {"from": "config", "to": "slot:workflow", "kind": "selects", "label": null},
                {"from": "slot:workflow", "to": "module:workflow:loop", "kind": "active_module", "label": null},
                {"from": "slot:workflow", "to": "ghost", "kind": "uses", "label": null},
            ],
            "warnings": []
        }))
        .expect("topology")
    }

    #[test]
    fn builds_slots_and_graph() {
        let snapshot = snapshot();
        let views = slot_views(&snapshot);
        assert_eq!(
            views[0].active.as_ref().map(|m| m.id.as_str()),
            Some("loop")
        );
        assert_eq!(views[0].alternatives.len(), 1);
        let graph = graph(&snapshot);
        assert!(graph.node("slot:context").is_some_and(|node| node.missing));
        assert!(
            graph
                .node("ghost")
                .is_some_and(|node| node.kind == Kind::Other)
        );
        assert_eq!(graph.related("slot:workflow").len(), 4);
    }

    #[test]
    fn layouts_hide_edges_to_unplaced_nodes() {
        let graph = graph(&snapshot());
        let assembly = layout(&graph, Scope::Assembly, false);
        assert_eq!(assembly.edges, vec![0, 2]);
        let modules = layout(&graph, Scope::Modules, false);
        assert_eq!(modules.nodes.len(), 2);
        assert_eq!(layout(&graph, Scope::Modules, true).nodes.len(), 4);
    }
}
