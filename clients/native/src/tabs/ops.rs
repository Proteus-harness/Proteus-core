//! Changes of the layout. Each one leaves a normalized tree: no empty panes
//! besides a lone root, no single-child or nested same-axis splits.

use super::is_chat;
use super::layout::TabLayout;
use super::tree::{Edge, Pane, PaneId};

impl TabLayout {
    pub fn focus(&mut self, pane: PaneId) {
        if self.pane(pane).is_some() {
            self.focused = pane;
        }
    }

    pub fn activate(&mut self, pane: PaneId, id: &str) {
        let Some(target) = self.root.pane_mut(pane) else {
            return;
        };
        if target.has(id) {
            target.show(id);
            self.focused = pane;
        }
    }

    /// Shows `id` where it is open, or opens it. A chat joins the chat in
    /// view; a document avoids covering a chat: it goes to another pane, or
    /// splits the only one when `can_split` says it is wide enough.
    pub fn open(&mut self, id: &str, can_split: bool) {
        if let Some(pane) = self.pane_of(id) {
            self.activate(pane, id);
            return;
        }
        match self.open_target(id, can_split) {
            Some(pane) => self.put(id, pane, None),
            None => {
                let focused = self.focused;
                let pane = self.new_pane(id);
                self.root.insert_beside(focused, Edge::Right, pane);
            }
        }
    }

    /// Pane a new tab goes to; `None` splits the focused pane.
    fn open_target(&self, id: &str, can_split: bool) -> Option<PaneId> {
        let focused = self.focused;
        let shows_chat = |pane: PaneId| self.active(pane).is_some_and(is_chat);
        let others: Vec<PaneId> = self
            .panes()
            .into_iter()
            .map(|pane| pane.id)
            .filter(|pane| *pane != focused)
            .collect();
        if is_chat(id) {
            if !shows_chat(focused)
                && let Some(pane) = others.iter().copied().find(|pane| shows_chat(*pane))
            {
                return Some(pane);
            }
            return Some(focused);
        }
        if !shows_chat(focused) {
            return Some(focused);
        }
        if let Some(pane) = others.iter().copied().find(|pane| !shows_chat(*pane)) {
            return Some(pane);
        }
        let neighbor = [Edge::Right, Edge::Left, Edge::Bottom, Edge::Top]
            .into_iter()
            .find_map(|edge| self.neighbor(focused, edge));
        match neighbor {
            Some(pane) => Some(pane),
            None if can_split => None,
            None => Some(focused),
        }
    }

    /// Opens `id` in place of the preview tab, or like [`Self::open`] when
    /// there is none; `id` becomes the preview. An open tab just comes
    /// forward and keeps its kind.
    pub fn open_preview(&mut self, id: &str, can_split: bool) {
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
                let ix = target
                    .tabs
                    .iter()
                    .position(|tab| *tab == old)
                    .expect("tab is open");
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

    pub fn close(&mut self, id: &str) {
        let before = self.pane_ids();
        self.detach(id);
        self.settle(&before);
    }

    /// Moves a tab in front of `before` in `pane`, or to its end.
    pub fn move_tab(&mut self, id: &str, pane: PaneId, before: Option<&str>) {
        if !self.contains(id) || self.pane(pane).is_none() {
            return;
        }
        if before == Some(id) {
            self.activate(pane, id);
            return;
        }
        let ids = self.pane_ids();
        self.detach(id);
        self.put(id, pane, before);
        self.settle(&ids);
    }

    /// Moves a tab into a new pane at `edge` of `pane`.
    pub fn split(&mut self, id: &str, pane: PaneId, edge: Edge) {
        if !self.can_split(id, pane) {
            return;
        }
        let ids = self.pane_ids();
        self.detach(id);
        let new = self.new_pane(id);
        self.root.insert_beside(pane, edge, new);
        self.settle(&ids);
    }

    /// Splits the active tab of the focused pane off to `edge`.
    pub fn split_active(&mut self, edge: Edge) {
        if let Some(id) = self.active(self.focused).map(str::to_string) {
            self.split(&id, self.focused, edge);
        }
    }

    /// Closes a pane; its tabs join the pane before it, or after it.
    pub fn close_pane(&mut self, pane: PaneId) {
        let ids = self.pane_ids();
        let Some(ix) = ids.iter().position(|id| *id == pane) else {
            return;
        };
        let Some(&target) = ids.get(ix.wrapping_sub(1)).or_else(|| ids.get(ix + 1)) else {
            return;
        };
        let closed = self.root.pane_mut(pane).expect("pane exists");
        let tabs = std::mem::take(&mut closed.tabs);
        let active = closed.active.take();
        let merged = self.root.pane_mut(target).expect("pane exists");
        merged.tabs.extend(tabs);
        if self.focused == pane
            && let Some(active) = active
        {
            merged.show(&active);
        }
        if merged.active.is_none() {
            merged.active = merged.tabs.first().cloned();
        }
        if self.focused == pane {
            self.focused = target;
        }
        self.settle(&ids);
    }

    /// Cycles the focused pane's tabs; returns the newly active one.
    pub fn step(&mut self, offset: isize) -> Option<String> {
        let pane = self.pane(self.focused)?;
        if pane.tabs.is_empty() {
            return None;
        }
        let current = pane
            .active
            .as_ref()
            .and_then(|active| pane.tabs.iter().position(|tab| tab == active))
            .unwrap_or(0);
        let next = (current as isize + offset).rem_euclid(pane.tabs.len() as isize) as usize;
        let id = pane.tabs[next].clone();
        self.activate(self.focused, &id);
        Some(id)
    }

    /// Moves the focus to the pane beyond `edge` of the focused one;
    /// returns that pane's active tab.
    pub fn focus_neighbor(&mut self, edge: Edge) -> Option<String> {
        let pane = self.neighbor(self.focused, edge)?;
        self.focused = pane;
        self.active(pane).map(str::to_string)
    }

    /// Closes tabs whose pages are gone, e.g. of a switched-off extension or
    /// a deleted session.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        let ids = self.pane_ids();
        let gone: Vec<String> = self
            .panes()
            .into_iter()
            .flat_map(|pane| pane.tabs.iter())
            .filter(|tab| !keep(tab))
            .cloned()
            .collect();
        for id in gone {
            self.detach(&id);
        }
        self.settle(&ids);
    }

    fn pane_ids(&self) -> Vec<PaneId> {
        self.panes().into_iter().map(|pane| pane.id).collect()
    }

    fn new_pane(&mut self, id: &str) -> Pane {
        let mut pane = Pane::new(PaneId(self.next_id));
        self.next_id += 1;
        pane.tabs.push(id.to_string());
        pane.active = Some(id.to_string());
        self.focused = pane.id;
        pane
    }

    /// Puts a tab that is not open into `pane` and shows it there.
    fn put(&mut self, id: &str, pane: PaneId, before: Option<&str>) {
        let target = self.root.pane_mut(pane).expect("pane exists");
        target.insert(id, before);
        target.show(id);
        self.focused = pane;
    }

    fn detach(&mut self, id: &str) {
        self.keep(id);
        if let Some(pane) = self.pane_of(id) {
            self.root.pane_mut(pane).expect("pane exists").remove(id);
        }
    }

    /// Normalizes the tree; when the focused pane is gone, the focus goes
    /// to the nearest surviving pane of the order `before` the change.
    fn settle(&mut self, before: &[PaneId]) {
        self.root.normalize();
        let now = self.pane_ids();
        if now.contains(&self.focused) {
            return;
        }
        let ix = before
            .iter()
            .position(|id| *id == self.focused)
            .unwrap_or(0);
        let earlier = before[..ix].iter().rev();
        let later = before.iter().skip(ix + 1);
        self.focused = earlier
            .chain(later)
            .copied()
            .find(|id| now.contains(id))
            .unwrap_or(now[0]);
    }
}
