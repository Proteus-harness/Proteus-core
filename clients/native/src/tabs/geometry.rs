//! Where panes are on screen and resizing of splits. Dividers are left
//! out: they are thin next to the panes.

use super::PANEL_WIDTH;
use super::layout::TabLayout;
use super::size::{Size, extents, set_extents};
use super::tree::{Axis, Edge, Node, PaneId};

/// Part of the window, in pixels or as fractions of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

const EPSILON: f32 = 1e-4;

/// Window size in pixels the arrangement is read at when only it matters.
const REFERENCE: (f32, f32) = (1600., 1000.);

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
            let total = match split.axis {
                Axis::Row => rect.w,
                Axis::Column => rect.h,
            };
            let mut offset = 0.;
            for (child, len) in split.children.iter().zip(extents(&split.children, total)) {
                let part = match split.axis {
                    Axis::Row => Rect {
                        x: rect.x + offset,
                        w: len,
                        ..rect
                    },
                    Axis::Column => Rect {
                        y: rect.y + offset,
                        h: len,
                        ..rect
                    },
                };
                offset += len;
                place(&child.node, part, out);
            }
        }
    }
}

impl TabLayout {
    /// Every pane with its place in pixels when the whole tree is
    /// `width` × `height`, in reading order.
    pub fn rects_in(&self, width: f32, height: f32) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        let whole = Rect {
            x: 0.,
            y: 0.,
            w: width,
            h: height,
        };
        place(&self.root, whole, &mut out);
        out
    }

    /// Every pane with its place as fractions of the tree, for a window of
    /// a typical size: fixed panes make the arrangement depend on it.
    pub fn rects(&self) -> Vec<(PaneId, Rect)> {
        let (width, height) = REFERENCE;
        let fraction = |rect: Rect| Rect {
            x: rect.x / width,
            y: rect.y / height,
            w: rect.w / width,
            h: rect.h / height,
        };
        self.rects_in(width, height)
            .into_iter()
            .map(|(id, rect)| (id, fraction(rect)))
            .collect()
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
    /// `at` pixels into the split, which has `total` pixels for its
    /// children. Each pane keeps at least `min` pixels.
    pub fn resize(&mut self, path: &[usize], index: usize, at: f32, total: f32, min: f32) {
        let Some(split) = self.root.split_at_mut(path) else {
            return;
        };
        if index + 1 >= split.children.len() {
            return;
        }
        let mut lengths = extents(&split.children, total);
        let start: f32 = lengths[..index].iter().sum();
        let pair = lengths[index] + lengths[index + 1];
        let min_a = split.children[index].node.min_panes(split.axis) as f32 * min;
        let min_b = split.children[index + 1].node.min_panes(split.axis) as f32 * min;
        let at = if min_a + min_b <= pair {
            at.clamp(start + min_a, start + pair - min_b)
        } else {
            start + pair * min_a / (min_a + min_b)
        };
        lengths[index] = at - start;
        lengths[index + 1] = pair - lengths[index];
        set_extents(&mut split.children, &lengths, &[index, index + 1]);
    }

    /// Gives every weighted child of the split at `path` the same share and
    /// fixed ones the width of a panel pane.
    pub fn reset(&mut self, path: &[usize]) {
        if let Some(split) = self.root.split_at_mut(path) {
            for child in &mut split.children {
                child.size = match child.size {
                    Size::Weight(_) => Size::Weight(1.),
                    Size::Fixed(_) => Size::Fixed(PANEL_WIDTH),
                };
            }
        }
    }

    /// Sizes of the children of the split at `path`.
    pub fn sizes(&self, path: &[usize]) -> Option<Vec<Size>> {
        let split = self.root.split_at(path)?;
        Some(split.children.iter().map(|child| child.size).collect())
    }

    /// Puts back sizes taken by [`Self::sizes`]; ignored when the split
    /// changed meanwhile.
    pub fn set_sizes(&mut self, path: &[usize], sizes: &[Size]) {
        let Some(split) = self.root.split_at_mut(path) else {
            return;
        };
        let fits = split.children.len() == sizes.len()
            && sizes.iter().all(|size| size.is_valid())
            && sizes.iter().any(|size| !size.is_fixed())
            && split
                .children
                .iter()
                .zip(sizes)
                .all(|(child, size)| child.size.is_fixed() == size.is_fixed());
        if !fits {
            return;
        }
        for (child, size) in split.children.iter_mut().zip(sizes) {
            child.size = *size;
        }
    }
}
