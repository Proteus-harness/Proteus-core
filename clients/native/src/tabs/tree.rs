//! The tree of areas: panes with tabs, and splits that share their space
//! between children.

use serde::{Deserialize, Serialize};

use super::pane::Pane;
use super::size::{self, Size};

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
pub struct Split {
    pub axis: Axis,
    pub children: Vec<Child>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Child {
    pub size: Size,
    pub node: Node,
}

impl Child {
    fn weighted(node: Node) -> Self {
        Self {
            size: Size::Weight(1.),
            node,
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
    pub(super) fn path_to(&self, id: PaneId) -> Option<Vec<usize>> {
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

    /// Places `pane` at `edge` of the pane `target`. A weighted target
    /// gives it half of its share; next to a fixed one it shares the room
    /// of the weighted siblings.
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
            let size = match &mut child.size {
                Size::Weight(weight) => {
                    *weight /= 2.;
                    Size::Weight(*weight)
                }
                Size::Fixed(_) => Size::Weight(1.),
            };
            let at = if edge.leading() { ix } else { ix + 1 };
            let node = Node::Pane(pane);
            split.children.insert(at, Child { size, node });
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
            children: children.into_iter().map(Child::weighted).collect(),
        });
        true
    }

    /// Drops empty panes besides `keep`, collapses splits with one child and
    /// merges a split into its parent of the same axis; sizes of each split
    /// are balanced. A weighted child that empties beside only fixed
    /// siblings, some of them locked, leaves an empty pane holding its room,
    /// so that they keep their pixels. A tree left without panes keeps its
    /// first one, empty.
    pub(super) fn normalize(&mut self, keep: Option<PaneId>) {
        let first = self.panes()[0].id;
        let tree = std::mem::replace(self, Node::Pane(Pane::new(first)));
        if let Some(tree) = tree.normalized(keep) {
            *self = tree;
        }
    }

    fn normalized(self, keep: Option<PaneId>) -> Option<Node> {
        let split = match self {
            Node::Pane(pane) => {
                return (!pane.tabs.is_empty() || Some(pane.id) == keep)
                    .then_some(Node::Pane(pane));
            }
            Node::Split(split) => split,
        };
        let axis = split.axis;
        let mut vacated = None;
        let mut entries: Vec<(Size, Option<Node>)> = Vec::with_capacity(split.children.len());
        for (ix, child) in split.children.into_iter().enumerate() {
            let first = child.node.panes()[0].id;
            let node = child.node.normalized(keep);
            if node.is_none() && !child.size.is_fixed() && vacated.is_none() {
                vacated = Some((ix, child.size, first));
            }
            entries.push((child.size, node));
        }
        size::hand_over(&mut entries);
        let mut children = Vec::with_capacity(entries.len());
        let mut slot = 0;
        for (ix, (size, node)) in entries.into_iter().enumerate() {
            if vacated.is_some_and(|(at, ..)| at == ix) {
                slot = children.len();
            }
            match node {
                None => {}
                Some(Node::Split(inner)) if inner.axis == axis => {
                    children.extend(size::merge(size, inner.children));
                }
                Some(node) => children.push(Child { size, node }),
            }
        }
        let locked = |child: &Child| matches!(&child.node, Node::Pane(pane) if pane.locked);
        if let Some((_, size, id)) = vacated
            && children.iter().all(|child| child.size.is_fixed())
            && children.iter().any(locked)
        {
            let node = Node::Pane(Pane::new(id));
            children.insert(slot, Child { size, node });
        }
        match children.len() {
            0 => None,
            1 => children.pop().map(|child| child.node),
            _ => {
                size::balance(&mut children);
                Some(Node::Split(Split { axis, children }))
            }
        }
    }

    /// Checks the shape of a split: enough children, usable sizes with at
    /// least one weighted child, no child split of the same axis.
    pub(super) fn validate(&self) -> Result<(), String> {
        let Node::Split(split) = self else {
            return Ok(());
        };
        if split.children.len() < 2 {
            return Err("a split needs at least two children".into());
        }
        if split.children.iter().all(|child| child.size.is_fixed()) {
            return Err("a split has no weighted child".into());
        }
        for child in &split.children {
            if !child.size.is_valid() {
                return Err(format!("size {:?} must be positive", child.size));
            }
            match &child.node {
                Node::Split(inner) if inner.axis == split.axis => {
                    return Err("a split holds a split of the same axis".into());
                }
                node => node.validate()?,
            }
        }
        Ok(())
    }
}
