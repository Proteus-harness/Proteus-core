//! The tree of areas: panes with tabs, and splits that share their space
//! between children by weight.

use serde::{Deserialize, Serialize};

/// Stable id of a pane; survives moves and normalization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaneId(pub u32);

/// How a split places its children: side by side or stacked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Row,
    Column,
}

/// Side of a pane: where a new pane appears, or where to look for a
/// neighbor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    pub fn axis(self) -> Axis {
        match self {
            Edge::Left | Edge::Right => Axis::Row,
            Edge::Top | Edge::Bottom => Axis::Column,
        }
    }

    /// Whether the side comes first along its axis.
    fn leading(self) -> bool {
        matches!(self, Edge::Left | Edge::Top)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Node {
    Pane(Pane),
    Split(Split),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pane {
    pub id: PaneId,
    pub tabs: Vec<String>,
    pub active: Option<String>,
    /// Shown again when the active tab closes.
    #[serde(skip)]
    pub previous: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Split {
    pub axis: Axis,
    pub children: Vec<Child>,
}

/// A child of a split; its share of the split is `weight` over the sum of
/// the weights.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Child {
    pub weight: f32,
    pub node: Node,
}

impl Pane {
    pub fn new(id: PaneId) -> Self {
        Self {
            id,
            tabs: Vec::new(),
            active: None,
            previous: None,
        }
    }

    pub fn has(&self, id: &str) -> bool {
        self.tabs.iter().any(|tab| tab == id)
    }

    pub fn show(&mut self, id: &str) {
        if self.has(id) && self.active.as_deref() != Some(id) {
            self.previous = self.active.replace(id.to_string());
        }
    }

    /// Puts a tab in front of `before`, or at the end.
    pub(super) fn insert(&mut self, id: &str, before: Option<&str>) {
        let at = before
            .and_then(|before| self.tabs.iter().position(|tab| tab == before))
            .unwrap_or(self.tabs.len());
        self.tabs.insert(at, id.to_string());
    }

    /// Takes a tab out; the previously shown one, else its left neighbor,
    /// becomes active.
    pub(super) fn remove(&mut self, id: &str) {
        let Some(ix) = self.tabs.iter().position(|tab| tab == id) else {
            return;
        };
        self.tabs.remove(ix);
        if self.previous.as_deref() == Some(id) {
            self.previous = None;
        }
        if self.active.as_deref() == Some(id) {
            self.active = self
                .previous
                .take()
                .or_else(|| self.tabs.get(ix.saturating_sub(1)).cloned());
        }
    }
}

impl Node {
    /// Panes in reading order: left to right, top to bottom.
    pub fn panes(&self) -> Vec<&Pane> {
        let mut panes = Vec::new();
        self.collect(&mut panes);
        panes
    }

    fn collect<'a>(&'a self, panes: &mut Vec<&'a Pane>) {
        match self {
            Node::Pane(pane) => panes.push(pane),
            Node::Split(split) => {
                for child in &split.children {
                    child.node.collect(panes);
                }
            }
        }
    }

    pub(super) fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        match self {
            Node::Pane(pane) => (pane.id == id).then_some(pane),
            Node::Split(split) => split
                .children
                .iter_mut()
                .find_map(|child| child.node.pane_mut(id)),
        }
    }

    /// Child indices from this node down to a pane.
    fn path_to(&self, id: PaneId) -> Option<Vec<usize>> {
        match self {
            Node::Pane(pane) => (pane.id == id).then(Vec::new),
            Node::Split(split) => split.children.iter().enumerate().find_map(|(ix, child)| {
                let mut path = child.node.path_to(id)?;
                path.insert(0, ix);
                Some(path)
            }),
        }
    }

    fn node_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        match path.split_first() {
            None => Some(self),
            Some((&ix, rest)) => match self {
                Node::Split(split) => split.children.get_mut(ix)?.node.node_mut(rest),
                Node::Pane(_) => None,
            },
        }
    }

    pub fn split_at(&self, path: &[usize]) -> Option<&Split> {
        let mut node = self;
        for &ix in path {
            match node {
                Node::Split(split) => node = &split.children.get(ix)?.node,
                Node::Pane(_) => return None,
            }
        }
        match node {
            Node::Split(split) => Some(split),
            Node::Pane(_) => None,
        }
    }

    pub(super) fn split_at_mut(&mut self, path: &[usize]) -> Option<&mut Split> {
        match self.node_mut(path)? {
            Node::Split(split) => Some(split),
            Node::Pane(_) => None,
        }
    }

    /// Fewest panes that line up along `axis` inside this node; each needs
    /// its minimum size.
    pub(super) fn min_panes(&self, axis: Axis) -> usize {
        match self {
            Node::Pane(_) => 1,
            Node::Split(split) => {
                let counts = split
                    .children
                    .iter()
                    .map(|child| child.node.min_panes(axis));
                if split.axis == axis {
                    counts.sum()
                } else {
                    counts.max().unwrap_or(1)
                }
            }
        }
    }

    fn is_empty_pane(&self) -> bool {
        matches!(self, Node::Pane(pane) if pane.tabs.is_empty())
    }

    /// Places `pane` at `edge` of the pane `target`, taking half of its
    /// space.
    pub(super) fn insert_beside(&mut self, target: PaneId, edge: Edge, pane: Pane) -> bool {
        let Some(path) = self.path_to(target) else {
            return false;
        };
        let axis = edge.axis();
        if let Some((&ix, parent)) = path.split_last()
            && let Some(split) = self.split_at_mut(parent)
            && split.axis == axis
        {
            let child = &mut split.children[ix];
            child.weight /= 2.;
            let weight = child.weight;
            let at = if edge.leading() { ix } else { ix + 1 };
            let node = Node::Pane(pane);
            split.children.insert(at, Child { weight, node });
            return true;
        }
        let slot = self.node_mut(&path).expect("pane is in the tree");
        let old = std::mem::replace(slot, Node::Pane(Pane::new(target)));
        let mut children = vec![old, Node::Pane(pane)];
        if edge.leading() {
            children.reverse();
        }
        *slot = Node::Split(Split {
            axis,
            children: children
                .into_iter()
                .map(|node| Child { weight: 1., node })
                .collect(),
        });
        true
    }

    /// Drops empty panes, collapses splits with one child and merges a
    /// split into its parent of the same axis; weights of each split are
    /// rescaled to average one. A tree without tabs keeps one empty pane.
    pub(super) fn normalize(&mut self) {
        let Node::Split(split) = self else {
            return;
        };
        for child in &mut split.children {
            child.node.normalize();
        }
        if split
            .children
            .iter()
            .all(|child| child.node.is_empty_pane())
        {
            let first = split.children.swap_remove(0).node;
            *self = first;
            return;
        }
        let axis = split.axis;
        let mut children = Vec::with_capacity(split.children.len());
        for child in split.children.drain(..) {
            match child.node {
                node if node.is_empty_pane() => {}
                Node::Split(inner) if inner.axis == axis => {
                    let total: f32 = inner.children.iter().map(|inner| inner.weight).sum();
                    children.extend(inner.children.into_iter().map(|inner| Child {
                        weight: child.weight * inner.weight / total,
                        node: inner.node,
                    }));
                }
                node => children.push(Child {
                    weight: child.weight,
                    node,
                }),
            }
        }
        if children.len() == 1 {
            *self = children.pop().expect("one child").node;
            return;
        }
        let scale = children.len() as f32 / children.iter().map(|c| c.weight).sum::<f32>();
        for child in &mut children {
            child.weight *= scale;
        }
        split.children = children;
    }

    /// Checks the shape of a split: enough children, usable weights, no
    /// child split of the same axis.
    pub(super) fn validate(&self) -> Result<(), String> {
        let Node::Split(split) = self else {
            return Ok(());
        };
        if split.children.len() < 2 {
            return Err("a split needs at least two children".into());
        }
        for child in &split.children {
            if !child.weight.is_finite() || child.weight <= 0. {
                return Err(format!("weight {} must be positive", child.weight));
            }
            match &child.node {
                Node::Split(inner) if inner.axis == split.axis => {
                    return Err("a split holds a split of the same axis".into());
                }
                node if node.is_empty_pane() => {
                    return Err("an empty pane is not the only one".into());
                }
                node => node.validate()?,
            }
        }
        Ok(())
    }
}
