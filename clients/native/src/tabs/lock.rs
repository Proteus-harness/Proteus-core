//! Locking a pane's size: a locked pane keeps its pixels along its split
//! when the window grows or its neighbours close, and is not split further;
//! an unlocked one keeps its share of the room.

use super::geometry::Rect;
use super::layout::TabLayout;
use super::size::{Size, balance, extents};
use super::tree::{Axis, Child, Node, PaneId};

impl TabLayout {
    /// Whether `pane` is locked, or `None` when locking means nothing: it
    /// fills the tree alone.
    pub fn pane_locked(&self, pane: PaneId) -> Option<bool> {
        let (children, ix) = self.siblings(pane)?;
        Some(self.pane(pane)?.locked && children[ix].size.is_fixed())
    }

    /// Whether `pane` is locked, so nothing splits it.
    pub fn is_locked(&self, pane: PaneId) -> bool {
        self.pane_locked(pane) == Some(true)
    }

    /// Locks or unlocks `pane` at the size it has when the tree is
    /// `width` × `height` with `gap` pixels per divider.
    pub fn lock_pane(&mut self, pane: PaneId, locked: bool, width: f32, height: f32, gap: f32) {
        if self.pane_locked(pane) != Some(!locked) {
            return;
        }
        let Some(path) = self.root.path_to(pane) else {
            return;
        };
        let Some((&ix, parent)) = path.split_last() else {
            return;
        };
        let whole = Rect {
            x: 0.,
            y: 0.,
            w: width,
            h: height,
        };
        let Some(total) = room(&self.root, parent, whole, gap) else {
            return;
        };
        let Some(split) = self.root.split_at_mut(parent) else {
            return;
        };
        let lengths = extents(&split.children, total);
        if locked {
            split.children[ix].size = Size::Fixed(lengths[ix].max(1.));
            // Someone has to take the rest: when no neighbour shares it, the
            // panels and locked panes beside it give up their pixels.
            let shares = |(at, child): (usize, &Child)| at != ix && !child.size.is_fixed();
            if !split.children.iter().enumerate().any(shares) {
                for (at, (child, len)) in split.children.iter_mut().zip(&lengths).enumerate() {
                    if at == ix {
                        continue;
                    }
                    child.size = Size::Weight(len.max(1.));
                    if let Node::Pane(neighbour) = &mut child.node {
                        neighbour.locked = false;
                    }
                }
            }
        } else {
            // Weights equal to the current lengths keep everything in place.
            for (at, (child, len)) in split.children.iter_mut().zip(&lengths).enumerate() {
                if at == ix || !child.size.is_fixed() {
                    child.size = Size::Weight(len.max(1.));
                }
            }
        }
        balance(&mut split.children);
        if let Some(target) = self.root.pane_mut(pane) {
            target.locked = locked;
        }
    }

    /// Forgets locks that stopped meaning anything after a change, e.g. of
    /// a pane left alone in the tree.
    pub(super) fn forget_stale_locks(&mut self) {
        let stale: Vec<PaneId> = self
            .panes()
            .into_iter()
            .filter(|pane| pane.locked)
            .map(|pane| pane.id)
            .filter(|&pane| !self.is_locked(pane))
            .collect();
        for pane in stale {
            if let Some(pane) = self.root.pane_mut(pane) {
                pane.locked = false;
            }
        }
    }

    /// The children of the split holding `pane`, and its index among them.
    fn siblings(&self, pane: PaneId) -> Option<(&[Child], usize)> {
        let path = self.root.path_to(pane)?;
        let (&ix, parent) = path.split_last()?;
        Some((&self.root.split_at(parent)?.children, ix))
    }
}

/// Pixels the children of the split at `path` share along its axis, inside
/// `rect` for `node`.
fn room(node: &Node, path: &[usize], rect: Rect, gap: f32) -> Option<f32> {
    let Node::Split(split) = node else {
        return None;
    };
    let gaps = gap * split.children.len().saturating_sub(1) as f32;
    let total = match split.axis {
        Axis::Row => rect.w,
        Axis::Column => rect.h,
    } - gaps;
    let Some((&ix, rest)) = path.split_first() else {
        return Some(total);
    };
    let lengths = extents(&split.children, total);
    let offset: f32 = lengths[..ix].iter().map(|len| len + gap).sum();
    let part = match split.axis {
        Axis::Row => Rect {
            x: rect.x + offset,
            w: lengths[ix],
            ..rect
        },
        Axis::Column => Rect {
            y: rect.y + offset,
            h: lengths[ix],
            ..rect
        },
    };
    room(&split.children.get(ix)?.node, rest, part, gap)
}
