//! Locking a pane's size: a locked pane keeps its pixels along its split
//! when the window grows or its neighbours close, an unlocked one keeps its
//! share of the room.

use super::geometry::Rect;
use super::layout::TabLayout;
use super::size::{Size, balance, extents};
use super::tree::{Axis, Node, PaneId};

impl TabLayout {
    /// Whether `pane` is locked, or `None` when locking means nothing: it
    /// fills the tree alone, or nothing beside it would take the rest.
    pub fn pane_locked(&self, pane: PaneId) -> Option<bool> {
        let path = self.root.path_to(pane)?;
        let (&ix, parent) = path.split_last()?;
        let split = self.root.split_at(parent)?;
        let locked = split.children[ix].size.is_fixed();
        let shared = split
            .children
            .iter()
            .enumerate()
            .any(|(at, child)| at != ix && !child.size.is_fixed());
        (locked || shared).then_some(locked)
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
        } else {
            // Weights equal to the current lengths keep everything in place.
            for (at, (child, len)) in split.children.iter_mut().zip(&lengths).enumerate() {
                if at == ix || !child.size.is_fixed() {
                    child.size = Size::Weight(len.max(1.));
                }
            }
        }
        balance(&mut split.children);
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
