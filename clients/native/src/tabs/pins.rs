//! Pinned tabs: kept in front of their pane and left alone by closing the
//! others.

use super::layout::TabLayout;

impl TabLayout {
    /// Pins or unpins a tab; a pinned preview becomes a regular tab.
    pub fn set_pinned(&mut self, id: &str, pinned: bool) {
        let Some(pane) = self.pane_of(id) else {
            return;
        };
        if pinned {
            self.keep(id);
        }
        self.root
            .pane_mut(pane)
            .expect("pane exists")
            .set_pinned(id, pinned);
    }

    /// Unpinned tabs of the pane of `id` besides `id` itself.
    pub fn others(&self, id: &str) -> Vec<String> {
        self.unpinned_around(id, |_| true)
    }

    /// Unpinned tabs to the right of `id` in its pane.
    pub fn right_of(&self, id: &str) -> Vec<String> {
        self.unpinned_around(id, |after| after)
    }

    fn unpinned_around(&self, id: &str, take: impl Fn(bool) -> bool) -> Vec<String> {
        let Some(pane) = self.pane_of(id).and_then(|pane| self.pane(pane)) else {
            return Vec::new();
        };
        let at = pane.position(id).expect("tab is open");
        pane.tabs
            .iter()
            .enumerate()
            .skip(pane.pinned)
            .filter(|(ix, _)| *ix != at && take(*ix > at))
            .map(|(_, tab)| tab.clone())
            .collect()
    }

    /// Closes the unpinned tabs of the pane of `id` besides `id`, which
    /// becomes active.
    pub fn close_others(&mut self, id: &str) {
        self.close_all(self.others(id), id);
    }

    /// Closes the unpinned tabs to the right of `id`, which becomes active.
    pub fn close_right(&mut self, id: &str) {
        self.close_all(self.right_of(id), id);
    }

    fn close_all(&mut self, tabs: Vec<String>, keep: &str) {
        let Some(pane) = self.pane_of(keep) else {
            return;
        };
        self.activate(pane, keep);
        let ids = self.pane_ids();
        for tab in tabs {
            self.detach(&tab);
        }
        self.settle(&ids);
    }
}
