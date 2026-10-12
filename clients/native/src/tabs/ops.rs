//! Changes of the layout. Each one leaves a normalized tree: no empty panes
//! besides one kept for chats and documents while there are none and ones
//! holding the room of locked neighbours, no single-child or nested
//! same-axis splits.

use super::layout::TabLayout;
use super::pane::Pane;
use super::tree::{Edge, PaneId};

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
        let pinned = self.detach(id);
        self.put(id, pane, before, pinned);
        self.settle(&ids);
    }

    /// Moves a tab into a new pane at `edge` of `pane`.
    pub fn split(&mut self, id: &str, pane: PaneId, edge: Edge) {
        if !self.can_split(id, pane) {
            return;
        }
        let ids = self.pane_ids();
        let pinned = self.detach(id);
        let new = self.new_pane(id, pinned);
        self.root.insert_beside(pane, edge, new);
        self.settle(&ids);
    }

    /// Splits the active tab of the focused pane off to `edge`.
    pub fn split_active(&mut self, edge: Edge) {
        if let Some(id) = self.active(self.focused).map(str::to_string) {
            self.split(&id, self.focused, edge);
        }
    }

    /// Closes a pane; its tabs join the pane before it, else the one after
    /// it. Its room goes to a neighbour even when they all are fixed.
    pub fn close_pane(&mut self, pane: PaneId) {
        if !self.can_close(pane) {
            return;
        }
        let ids = self.pane_ids();
        let Some(ix) = ids.iter().position(|id| *id == pane) else {
            return;
        };
        self.hand_room(pane);
        let before = ix.checked_sub(1).map(|at| ids[at]);
        let Some(target) = before.or_else(|| ids.get(ix + 1).copied()) else {
            return;
        };
        let closed = self.root.pane_mut(pane).expect("pane exists");
        let tabs = std::mem::take(&mut closed.tabs);
        let pinned = std::mem::take(&mut closed.pinned);
        let active = closed.active.take();
        let merged = self.root.pane_mut(target).expect("pane exists");
        for (ix, tab) in tabs.iter().enumerate() {
            merged.insert(tab, None, ix < pinned);
        }
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

    /// Whether closing `pane` changes anything: not for the last pane or
    /// the empty one kept for chats and documents.
    pub fn can_close(&self, pane: PaneId) -> bool {
        let has_tabs = self.pane(pane).is_some_and(|pane| !pane.tabs.is_empty());
        self.pane_count() > 1 && (has_tabs || self.placeholder() != Some(pane))
    }

    /// The empty pane that stays for chats and documents when no pane has
    /// any: a weighted one, so fixed ones keep their pixels, then the
    /// focused one, else the first.
    fn placeholder(&self) -> Option<PaneId> {
        if self.panes().iter().any(|pane| !pane.tabs.is_empty()) {
            return None;
        }
        let fixed = |pane: PaneId| {
            self.siblings(pane)
                .is_some_and(|(children, ix)| children[ix].size.is_fixed())
        };
        let empty = self.panes().into_iter().filter(|pane| pane.tabs.is_empty());
        empty
            .min_by_key(|pane| (fixed(pane.id), pane.id != self.focused))
            .map(|pane| pane.id)
    }

    pub(super) fn pane_ids(&self) -> Vec<PaneId> {
        self.panes().into_iter().map(|pane| pane.id).collect()
    }

    pub(super) fn new_pane(&mut self, id: &str, pinned: bool) -> Pane {
        let mut pane = Pane::new(PaneId(self.next_id));
        self.next_id += 1;
        pane.tabs.push(id.to_string());
        pane.pinned = usize::from(pinned);
        pane.active = Some(id.to_string());
        self.focused = pane.id;
        pane
    }

    /// Puts a tab that is not open into `pane` and shows it there.
    pub(super) fn put(&mut self, id: &str, pane: PaneId, before: Option<&str>, pinned: bool) {
        let target = self.root.pane_mut(pane).expect("pane exists");
        target.insert(id, before, pinned);
        target.show(id);
        self.focused = pane;
    }

    /// Takes a tab out of its pane; returns whether it was pinned.
    pub(super) fn detach(&mut self, id: &str) -> bool {
        self.keep(id);
        match self.pane_of(id) {
            Some(pane) => self.root.pane_mut(pane).expect("pane exists").remove(id),
            None => false,
        }
    }

    /// Normalizes the tree; when the focused pane is gone, the focus goes
    /// to the nearest surviving pane of the order `before` the change.
    pub(super) fn settle(&mut self, before: &[PaneId]) {
        let keep = self.placeholder();
        self.root.normalize(keep);
        self.forget_stale_locks();
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
