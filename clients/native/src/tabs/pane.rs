//! A pane: its tabs, the pinned ones first, and which one is shown.

use serde::{Deserialize, Serialize};

use super::tree::PaneId;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pane {
    pub id: PaneId,
    pub tabs: Vec<String>,
    /// The first `pinned` tabs are pinned: they stay in front and are left
    /// out when closing the others.
    pub pinned: usize,
    pub active: Option<String>,
    /// The user locked the pane's size: it keeps its pixels and is not
    /// split further. See [`super::TabLayout::pane_locked`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// Shown again when the active tab closes.
    #[serde(skip)]
    pub previous: Option<String>,
}

impl Pane {
    pub fn new(id: PaneId) -> Self {
        Self {
            id,
            tabs: Vec::new(),
            pinned: 0,
            active: None,
            locked: false,
            previous: None,
        }
    }

    pub fn has(&self, id: &str) -> bool {
        self.tabs.iter().any(|tab| tab == id)
    }

    pub fn position(&self, id: &str) -> Option<usize> {
        self.tabs.iter().position(|tab| tab == id)
    }

    pub fn is_pinned(&self, id: &str) -> bool {
        self.position(id).is_some_and(|ix| ix < self.pinned)
    }

    pub fn show(&mut self, id: &str) {
        if self.has(id) && self.active.as_deref() != Some(id) {
            self.previous = self.active.replace(id.to_string());
        }
    }

    /// Puts a tab in front of `before`, or at the end of its group: pinned
    /// tabs stay among the pinned ones, others after them.
    pub(super) fn insert(&mut self, id: &str, before: Option<&str>, pinned: bool) {
        let (start, end) = if pinned {
            (0, self.pinned)
        } else {
            (self.pinned, self.tabs.len())
        };
        let at = before
            .and_then(|before| self.position(before))
            .unwrap_or(end)
            .clamp(start, end);
        self.tabs.insert(at, id.to_string());
        if pinned {
            self.pinned += 1;
        }
    }

    /// Takes a tab out; the previously shown one, else its left neighbor,
    /// becomes active. Returns whether the tab was pinned.
    pub(super) fn remove(&mut self, id: &str) -> bool {
        let Some(ix) = self.position(id) else {
            return false;
        };
        self.tabs.remove(ix);
        let pinned = ix < self.pinned;
        if pinned {
            self.pinned -= 1;
        }
        if self.previous.as_deref() == Some(id) {
            self.previous = None;
        }
        if self.active.as_deref() == Some(id) {
            self.active = self
                .previous
                .take()
                .or_else(|| self.tabs.get(ix.saturating_sub(1)).cloned());
        }
        pinned
    }

    /// Moves a tab to the end of the pinned tabs, or to the start of the
    /// others; the shown tab stays shown.
    pub(super) fn set_pinned(&mut self, id: &str, pinned: bool) {
        let Some(ix) = self.position(id) else {
            return;
        };
        if (ix < self.pinned) == pinned {
            return;
        }
        let tab = self.tabs.remove(ix);
        if pinned {
            self.tabs.insert(self.pinned, tab);
            self.pinned += 1;
        } else {
            self.pinned -= 1;
            self.tabs.insert(self.pinned, tab);
        }
    }
}
