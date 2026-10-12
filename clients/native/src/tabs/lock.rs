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
            // fixed panes beside it give up their pixels.
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
        // An empty pane that held the room of a pane now unlocked goes.
        let ids = self.pane_ids();
        self.settle(&ids);
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

    /// Whether `pane` is empty and holds the room of fixed neighbours, some
    /// of them locked, which would grow without it.
    pub fn holds_room(&self, pane: PaneId) -> bool {
        let Some((children, ix)) = self.siblings(pane) else {
            return false;
        };
        let others = || {
            let others = children.iter().enumerate().filter(move |(at, _)| *at != ix);
            others.map(|(_, child)| child)
        };
        let locked = |child: &Child| matches!(&child.node, Node::Pane(pane) if pane.locked);
        self.pane(pane).is_some_and(|pane| pane.tabs.is_empty())
            && others().all(|child| child.size.is_fixed())
            && others().any(locked)
    }

    /// Before `pane` closes, makes sure a neighbour takes its room: when all
    /// of them are fixed, the nearest unlocked one shares the rest from now
    /// on, else the nearest locked one, which loses its lock.
    pub(super) fn hand_room(&mut self, pane: PaneId) {
        let Some(path) = self.root.path_to(pane) else {
            return;
        };
        let Some((&ix, parent)) = path.split_last() else {
            return;
        };
        let Some(split) = self.root.split_at_mut(parent) else {
            return;
        };
        let others = || (0..split.children.len()).filter(move |&at| at != ix);
        if others().any(|at| !split.children[at].size.is_fixed()) {
            return;
        }
        let locked = |child: &Child| matches!(&child.node, Node::Pane(pane) if pane.locked);
        let Some(heir) = others().min_by_key(|&at| (locked(&split.children[at]), at.abs_diff(ix)))
        else {
            return;
        };
        let child = &mut split.children[heir];
        child.size = Size::Weight(1.);
        if let Node::Pane(neighbour) = &mut child.node {
            neighbour.locked = false;
        }
    }

    /// The children of the split holding `pane`, and its index among them.
    pub(super) fn siblings(&self, pane: PaneId) -> Option<(&[Child], usize)> {
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
