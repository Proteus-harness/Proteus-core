//! Opening tabs: where a new chat, document or panel goes, and the preview
//! tab that browsing replaces.

use super::layout::TabLayout;
use super::size::Size;
use super::tree::{Edge, PaneId};
use super::{PANEL_WIDTH, is_chat, is_panel};

/// Where [`TabLayout::open`] puts a new chat or document.
enum Target {
    Pane(PaneId),
    /// A new pane to the right of this one.
    Split(PaneId),
    /// A new pane at the right of the whole tree: there is no pane for
    /// chats and documents.
    Attach,
}

impl TabLayout {
    /// Shows `id` where it is open, or opens it. A panel goes to the panels
    /// (see [`Self::open_panel`]). A chat joins the chat in view; a document
    /// avoids covering a chat: it goes to another pane, or splits the pane
    /// when `can_split` says it is wide enough. Neither goes to a pane that
    /// holds only panels.
    pub fn open(&mut self, id: &str, can_split: impl Fn(PaneId) -> bool) {
        if let Some(pane) = self.pane_of(id) {
            self.activate(pane, id);
            return;
        }
        if is_panel(id) {
            self.open_panel(id, None);
            return;
        }
        match self.open_target(id, can_split) {
            Target::Pane(pane) => self.put(id, pane, None, false),
            Target::Split(pane) => {
                let new = self.new_pane(id, false);
                self.root.insert_beside(pane, Edge::Right, new);
            }
            Target::Attach => {
                let new = self.new_pane(id, false);
                self.root
                    .attach(new, false, Size::Weight(1.), Size::Fixed(PANEL_WIDTH));
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
        let mut panes: Vec<PaneId> = self.content_panes().map(|pane| pane.id).collect();
        if let Some(ix) = panes.iter().position(|pane| *pane == self.focused) {
            panes[..=ix].rotate_right(1);
        }
        let Some(&base) = panes.first() else {
            return Target::Attach;
        };
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

    /// Shows the panel `id` where it is open, or opens it pinned in `pane`;
    /// by default in the focused or first pane holding only panels, else
    /// in a new fixed pane on the left.
    pub fn open_panel(&mut self, id: &str, pane: Option<PaneId>) {
        if let Some(open) = self.pane_of(id) {
            self.activate(open, id);
            return;
        }
        let holds_panels = |pane: PaneId| self.pane(pane).is_some_and(|pane| pane.holds_panels());
        let target = pane.filter(|pane| self.pane(*pane).is_some()).or_else(|| {
            let panels = self.panes().into_iter().filter(|pane| pane.holds_panels());
            let first = panels.map(|pane| pane.id).next();
            Some(self.focused)
                .filter(|pane| holds_panels(*pane))
                .or(first)
        });
        match target {
            Some(pane) => self.put(id, pane, None, true),
            None => {
                let new = self.new_pane(id, true);
                self.root
                    .attach(new, true, Size::Fixed(PANEL_WIDTH), Size::Weight(1.));
            }
        }
    }

    /// Opens `id` in place of the preview tab, or like [`Self::open`] when
    /// there is none; `id` becomes the preview. An open tab just comes
    /// forward and keeps its kind; a panel is never a preview.
    pub fn open_preview(&mut self, id: &str, can_split: impl Fn(PaneId) -> bool) {
        if let Some(pane) = self.pane_of(id) {
            self.activate(pane, id);
            return;
        }
        if is_panel(id) {
            self.open_panel(id, None);
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
