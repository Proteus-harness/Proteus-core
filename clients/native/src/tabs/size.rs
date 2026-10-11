//! How a child of a split claims its room: a share of what fixed children
//! leave, or a fixed number of pixels that resizing the window keeps.

use serde::{Deserialize, Serialize};

use super::tree::Child;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Size {
    /// Share of the room left by fixed children: the weight over the sum of
    /// the weights.
    Weight(f32),
    /// Pixels along the split's axis.
    Fixed(f32),
}

impl Size {
    pub fn is_valid(self) -> bool {
        let value = match self {
            Size::Weight(value) | Size::Fixed(value) => value,
        };
        value.is_finite() && value > 0.
    }

    pub fn is_fixed(self) -> bool {
        matches!(self, Size::Fixed(_))
    }

    fn weight(self) -> f32 {
        match self {
            Size::Weight(weight) => weight,
            Size::Fixed(_) => 0.,
        }
    }
}

/// Lengths of `children` along a split `total` pixels long: fixed children
/// get their pixels, shrunk together when they do not fit, and weighted
/// ones share the rest.
pub(super) fn extents(children: &[Child], total: f32) -> Vec<f32> {
    let fixed: f32 = children
        .iter()
        .map(|child| match child.size {
            Size::Fixed(px) => px,
            Size::Weight(_) => 0.,
        })
        .sum();
    let scale = if fixed > total && fixed > 0. {
        total / fixed
    } else {
        1.
    };
    let rest = (total - fixed).max(0.);
    let weights: f32 = children.iter().map(|child| child.size.weight()).sum();
    children
        .iter()
        .map(|child| match child.size {
            Size::Fixed(px) => px * scale,
            Size::Weight(weight) => rest * weight / weights,
        })
        .collect()
}

/// Gives children the lengths `lengths`: fixed children among `changed`
/// take their new pixels, weighted ones weights in proportion to their
/// lengths. Returns `false`, changing nothing, when a length is not usable.
pub(super) fn set_extents(children: &mut [Child], lengths: &[f32], changed: &[usize]) -> bool {
    if children.len() != lengths.len() {
        return false;
    }
    let usable = children
        .iter()
        .zip(lengths)
        .enumerate()
        .all(|(ix, (child, len))| {
            let needed = !child.size.is_fixed() || changed.contains(&ix);
            !needed || (len.is_finite() && *len > 0.)
        });
    if !usable {
        return false;
    }
    for (ix, (child, len)) in children.iter_mut().zip(lengths).enumerate() {
        child.size = match child.size {
            Size::Weight(_) => Size::Weight(*len),
            Size::Fixed(_) if changed.contains(&ix) => Size::Fixed(*len),
            fixed => fixed,
        };
    }
    balance(children);
    true
}

/// Keeps a split resizable with the window: without weighted children the
/// fixed ones turn into weights in proportion to their pixels. Weights are
/// rescaled to average one.
pub(super) fn balance(children: &mut [Child]) {
    if children.iter().all(|child| child.size.is_fixed()) {
        for child in children.iter_mut() {
            if let Size::Fixed(px) = child.size {
                child.size = Size::Weight(px);
            }
        }
    }
    let weighted = children.iter().filter(|c| !c.size.is_fixed()).count();
    let total: f32 = children.iter().map(|child| child.size.weight()).sum();
    for child in children {
        if let Size::Weight(weight) = &mut child.size {
            *weight = *weight * weighted as f32 / total;
        }
    }
}

/// Gives the share of each dropped weighted child (`None`) to its nearest
/// weighted sibling, the one before it first, so that closing a pane
/// widens its neighbour and leaves the other panes where they are.
pub(super) fn hand_over<T>(entries: &mut [(Size, Option<T>)]) {
    for ix in 0..entries.len() {
        let (Size::Weight(freed), None) = &entries[ix] else {
            continue;
        };
        let freed = *freed;
        let takes = |entry: &(Size, Option<T>)| entry.1.is_some() && !entry.0.is_fixed();
        let before = (0..ix).rev().find(|&at| takes(&entries[at]));
        let after = || (ix + 1..entries.len()).find(|&at| takes(&entries[at]));
        if let Some(heir) = before.or_else(after)
            && let Size::Weight(weight) = &mut entries[heir].0
        {
            *weight += freed;
        }
    }
}

/// Children of a split that sized `outer` in its parent of the same axis,
/// sized for the parent. Weighted children of a fixed split become fixed
/// at their current pixels.
pub(super) fn merge(outer: Size, inner: Vec<Child>) -> impl Iterator<Item = Child> {
    let weights: f32 = inner.iter().map(|child| child.size.weight()).sum();
    let lengths = match outer {
        Size::Fixed(px) => extents(&inner, px),
        Size::Weight(_) => Vec::new(),
    };
    inner.into_iter().enumerate().map(move |(ix, child)| {
        let size = match (outer, child.size) {
            (_, Size::Fixed(px)) => Size::Fixed(px),
            (Size::Weight(share), Size::Weight(weight)) => Size::Weight(share * weight / weights),
            (Size::Fixed(_), Size::Weight(_)) => Size::Fixed(lengths[ix].max(1.)),
        };
        Child {
            size,
            node: child.node,
        }
    })
}
