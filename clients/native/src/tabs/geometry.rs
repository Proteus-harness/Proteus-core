//! Where panes are on screen, as fractions of the main area, and resizing
//! of splits. Dividers are left out: they are thin next to the panes.

use super::layout::TabLayout;
use super::tree::{Axis, Edge, Node, PaneId};

/// Part of the main area, every coordinate within `0.0..=1.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

const EPSILON: f32 = 1e-4;

/// Share of a pane next to its edge where a dropped tab splits the pane
/// rather than joining it.
const EDGE_ZONE: f32 = 0.3;

impl Rect {
    fn right(self) -> f32 {
        self.x + self.w
    }

    fn bottom(self) -> f32 {
        self.y + self.h
    }
}

/// Edge of a pane a tab dropped at `(x, y)` (fractions of the pane) splits
/// off to; `None` in the middle, where it joins the pane.
pub fn drop_edge(x: f32, y: f32) -> Option<Edge> {
    [
        (x, Edge::Left),
        (1. - x, Edge::Right),
        (y, Edge::Top),
        (1. - y, Edge::Bottom),
    ]
    .into_iter()
    .filter(|(distance, _)| *distance < EDGE_ZONE)
    .min_by(|a, b| a.0.total_cmp(&b.0))
    .map(|(_, edge)| edge)
}

fn place(node: &Node, rect: Rect, out: &mut Vec<(PaneId, Rect)>) {
    match node {
        Node::Pane(pane) => out.push((pane.id, rect)),
        Node::Split(split) => {
            let total: f32 = split.children.iter().map(|child| child.weight).sum();
            let mut offset = 0.;
            for child in &split.children {
                let share = child.weight / total;
                let part = match split.axis {
                    Axis::Row => Rect {
                        x: rect.x + offset * rect.w,
                        w: share * rect.w,
                        ..rect
                    },
                    Axis::Column => Rect {
                        y: rect.y + offset * rect.h,
                        h: share * rect.h,
                        ..rect
                    },
                };
                offset += share;
                place(&child.node, part, out);
            }
        }
    }
}

impl TabLayout {
    /// Every pane with its place, in reading order.
    pub fn rects(&self) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        let whole = Rect {
            x: 0.,
            y: 0.,
            w: 1.,
            h: 1.,
        };
        place(&self.root, whole, &mut out);
        out
    }

    pub fn rect(&self, pane: PaneId) -> Option<Rect> {
        self.rects()
            .into_iter()
            .find(|(id, _)| *id == pane)
            .map(|(_, rect)| rect)
    }

    /// The pane right beyond `edge` of `pane`: the closest one along the
    /// way, then the one sharing the longest border.
    pub fn neighbor(&self, pane: PaneId, edge: Edge) -> Option<PaneId> {
        let from = self.rect(pane)?;
        let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| a1.min(b1) - a0.max(b0);
        self.rects()
            .into_iter()
            .filter(|(id, _)| *id != pane)
            .filter_map(|(id, to)| {
                let (gap, shared) = match edge {
                    Edge::Right => (
                        to.x - from.right(),
                        overlap(from.y, from.bottom(), to.y, to.bottom()),
                    ),
                    Edge::Left => (
                        from.x - to.right(),
                        overlap(from.y, from.bottom(), to.y, to.bottom()),
                    ),
                    Edge::Bottom => (
                        to.y - from.bottom(),
                        overlap(from.x, from.right(), to.x, to.right()),
                    ),
                    Edge::Top => (
                        from.y - to.bottom(),
                        overlap(from.x, from.right(), to.x, to.right()),
                    ),
                };
                (gap > -EPSILON && shared > EPSILON).then_some((id, gap, shared))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(b.2.total_cmp(&a.2)))
            .map(|(id, _, _)| id)
    }

    /// Moves the divider after child `index` of the split at `path` to
    /// `at`, a fraction of the split along its axis. Each pane keeps at
    /// least `unit` of the split.
    pub fn resize(&mut self, path: &[usize], index: usize, at: f32, unit: f32) {
        let Some(split) = self.root.split_at_mut(path) else {
            return;
        };
        if index + 1 >= split.children.len() {
            return;
        }
        let total: f32 = split.children.iter().map(|child| child.weight).sum();
        let before: f32 = split.children[..index].iter().map(|c| c.weight).sum();
        let pair = split.children[index].weight + split.children[index + 1].weight;
        let start = before / total;
        let end = (before + pair) / total;
        let min_a = split.children[index].node.min_panes(split.axis) as f32 * unit;
        let min_b = split.children[index + 1].node.min_panes(split.axis) as f32 * unit;
        let at = if start + min_a <= end - min_b {
            at.clamp(start + min_a, end - min_b)
        } else {
            start + (end - start) * min_a / (min_a + min_b)
        };
        let first = (at - start) * total;
        if first > 0. && first < pair {
            split.children[index].weight = first;
            split.children[index + 1].weight = pair - first;
        }
    }

    /// Gives every child of the split at `path` the same share.
    pub fn reset(&mut self, path: &[usize]) {
        if let Some(split) = self.root.split_at_mut(path) {
            for child in &mut split.children {
                child.weight = 1.;
            }
        }
    }

    /// Weights of the children of the split at `path`.
    pub fn weights(&self, path: &[usize]) -> Option<Vec<f32>> {
        let split = self.root.split_at(path)?;
        Some(split.children.iter().map(|child| child.weight).collect())
    }

    /// Puts back weights taken by [`Self::weights`]; ignored when the
    /// split changed meanwhile.
    pub fn set_weights(&mut self, path: &[usize], weights: &[f32]) {
        let Some(split) = self.root.split_at_mut(path) else {
            return;
        };
        if split.children.len() != weights.len()
            || weights.iter().any(|w| !w.is_finite() || *w <= 0.)
        {
            return;
        }
        for (child, weight) in split.children.iter_mut().zip(weights) {
            child.weight = *weight;
        }
    }
}
