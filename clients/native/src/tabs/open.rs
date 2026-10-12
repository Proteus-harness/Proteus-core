//! Opening tabs: where a new chat or document goes, and the preview tab
//! that browsing replaces.

use super::is_chat;
use super::layout::TabLayout;
use super::tree::{Edge, PaneId};

/// Where [`TabLayout::open`] puts a new chat or document.
enum Target {
    Pane(PaneId),
    /// A new pane to the right of this one.
    Split(PaneId),
}

impl TabLayout {
    /// Shows `id` where it is open, or opens it. An empty pane takes it
    /// first. A chat joins the chat in view; a document avoids covering a
    /// chat: it goes to another pane, or splits the pane when `can_split`
    /// says it is wide enough.
    pub fn open(&mut self, id: &str, can_split: impl Fn(PaneId) -> bool) {
        if let Some(pane) = self.pane_of(id) {
            self.activate(pane, id);
            return;
        }
        match self.open_target(id, can_split) {
            Target::Pane(pane) => self.put(id, pane, None, false),
            Target::Split(pane) => {
                let new = self.new_pane(id, false);
                self.root.insert_beside(pane, Edge::Right, new);
            }
        }
    }

    /// Opens `id` in `pane`, chosen by the user, or shows it where it is
    /// open; without that pane it goes where [`Self::open`] puts it.
    pub fn open_in(&mut self, id: &str, pane: PaneId, can_split: impl Fn(PaneId) -> bool) {
        if let Some(open) = self.pane_of(id) {
            self.activate(open, id);
        } else if self.pane(pane).is_some() {
            self.put(id, pane, None, false);
        } else {
            self.open(id, can_split);
        }
    }

    fn open_target(&self, id: &str, can_split: impl Fn(PaneId) -> bool) -> Target {
        let shows_chat = |pane: PaneId| self.active(pane).is_some_and(is_chat);
        // The focused pane first, then the others in reading order.
        let mut panes = self.pane_ids();
        if let Some(ix) = panes.iter().position(|pane| *pane == self.focused) {
            panes[..=ix].rotate_right(1);
        }
        let base = panes[0];
        let empty = |pane: &PaneId| self.pane(*pane).is_some_and(|pane| pane.tabs.is_empty());
        if let Some(pane) = panes.iter().copied().find(empty) {
            return Target::Pane(pane);
        }
        if is_chat(id) {
            let pane = panes.iter().copied().find(|pane| shows_chat(*pane));
            return Target::Pane(pane.unwrap_or(base));
        }
        if let Some(pane) = panes.iter().copied().find(|pane| !shows_chat(*pane)) {
            return Target::Pane(pane);
        }
        let neighbor = [Edge::Right, Edge::Left, Edge::Bottom, Edge::Top]
            .into_iter()
            .filter_map(|edge| self.neighbor(base, edge))
            .find(|pane| panes.contains(pane));
        match neighbor {
            Some(pane) => Target::Pane(pane),
            None if can_split(base) && !self.is_locked(base) => Target::Split(base),
            None => Target::Pane(base),
        }
    }

    /// Opens `id` in place of the preview tab, or like [`Self::open`] when
    /// there is none; `id` becomes the preview. An open tab just comes
    /// forward and keeps its kind.
    pub fn open_preview(&mut self, id: &str, can_split: impl Fn(PaneId) -> bool) {
        if let Some(pane) = self.pane_of(id) {
            self.activate(pane, id);
            return;
        }
        let replaced = self
            .preview
            .take()
            .and_then(|old| Some((self.pane_of(&old)?, old)));
        match replaced {
            Some((pane, old)) => {
                let target = self.root.pane_mut(pane).expect("pane exists");
                let ix = target.position(&old).expect("tab is open");
                target.tabs[ix] = id.to_string();
                if target.previous.as_deref() == Some(old.as_str()) {
                    target.previous = None;
                }
                if target.active.as_deref() == Some(old.as_str()) {
                    target.active = Some(id.to_string());
                    self.focused = pane;
                } else {
                    self.activate(pane, id);
                }
            }
            None => self.open(id, can_split),
        }
        self.preview = Some(id.to_string());
    }

    /// Turns the preview tab `id` into a regular one.
    pub fn keep(&mut self, id: &str) {
        if self.preview.as_deref() == Some(id) {
            self.preview = None;
        }
    }
}
