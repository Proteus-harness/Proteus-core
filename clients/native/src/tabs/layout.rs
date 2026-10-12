//! The layout as a whole: the tree, which pane has the focus, the preview
//! tab, and what can be read from them.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::is_chat;
use super::pane::Pane;
use super::tree::{Node, PaneId};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabLayout {
    pub root: Node,
    pub focused: PaneId,
    /// Tab that the next previewed document replaces, so browsing does not
    /// pile up tabs. Keeping or moving it makes it a regular tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// Id of the next new pane.
    pub next_id: u32,
}

/// A fresh layout: one empty pane for chats and documents.
impl Default for TabLayout {
    fn default() -> Self {
        Self {
            root: Node::Pane(Pane::new(PaneId(0))),
            focused: PaneId(0),
            preview: None,
            next_id: 1,
        }
    }
}

impl TabLayout {
    pub fn validate(&self) -> Result<(), String> {
        self.root.validate()?;
        let mut ids = HashSet::new();
        let mut seen = HashSet::new();
        let panes = self.panes();
        for pane in &panes {
            if !ids.insert(pane.id) {
                return Err(format!("pane {} appears twice", pane.id.0));
            }
            if pane.id.0 >= self.next_id {
                return Err(format!("pane {} is not below next_id", pane.id.0));
            }
            if pane.pinned > pane.tabs.len() {
                return Err(format!("pane {} pins more tabs than it has", pane.id.0));
            }
            for tab in &pane.tabs {
                if !seen.insert(tab.as_str()) {
                    return Err(format!("tab `{tab}` is open twice"));
                }
            }
            match &pane.active {
                Some(active) if !pane.has(active) => {
                    return Err(format!("active tab `{active}` is not open in its pane"));
                }
                None if !pane.tabs.is_empty() => {
                    return Err("a pane with tabs has no active tab".into());
                }
                _ => {}
            }
        }
        let loose = panes
            .iter()
            .filter(|pane| pane.tabs.is_empty() && !self.holds_room(pane.id))
            .count();
        if loose > 1 {
            return Err("more than one pane is empty".into());
        }
        if loose == 1 && panes.iter().any(|pane| !pane.tabs.is_empty()) {
            return Err("an empty pane is next to chats or documents".into());
        }
        if !ids.contains(&self.focused) {
            return Err(format!("focused pane {} does not exist", self.focused.0));
        }
        match &self.preview {
            Some(preview) if !seen.contains(preview.as_str()) => {
                Err(format!("preview tab `{preview}` is not open"))
            }
            Some(preview) if self.is_pinned(preview) => {
                Err(format!("preview tab `{preview}` is pinned"))
            }
            _ => Ok(()),
        }
    }

    /// Panes in reading order.
    pub fn panes(&self) -> Vec<&Pane> {
        self.root.panes()
    }

    pub fn pane_count(&self) -> usize {
        self.panes().len()
    }

    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes().into_iter().find(|pane| pane.id == id)
    }

    pub fn pane_of(&self, tab: &str) -> Option<PaneId> {
        self.panes()
            .into_iter()
            .find(|pane| pane.has(tab))
            .map(|pane| pane.id)
    }

    pub fn contains(&self, tab: &str) -> bool {
        self.pane_of(tab).is_some()
    }

    pub fn is_pinned(&self, tab: &str) -> bool {
        self.panes().into_iter().any(|pane| pane.is_pinned(tab))
    }

    pub fn active(&self, pane: PaneId) -> Option<&str> {
        self.pane(pane)?.active.as_deref()
    }

    /// Tabs on screen: the active tab of every pane.
    pub fn visible(&self) -> impl Iterator<Item = &str> {
        self.panes()
            .into_iter()
            .filter_map(|pane| pane.active.as_deref())
    }

    /// Whether moving `tab` to an edge of `pane` changes anything and is
    /// allowed: a pane cannot be split off its only tab, a locked pane not
    /// at all.
    pub fn can_split(&self, tab: &str, pane: PaneId) -> bool {
        if self.is_locked(pane) {
            return false;
        }
        match self.pane(pane) {
            Some(target) if target.has(tab) => target.tabs.len() > 1,
            Some(_) => self.contains(tab),
            None => false,
        }
    }

    /// Open chat tabs, pane by pane.
    pub fn chats(&self) -> impl Iterator<Item = &str> {
        self.panes()
            .into_iter()
            .flat_map(|pane| pane.tabs.iter())
            .map(String::as_str)
            .filter(|id| is_chat(id))
    }

    /// The chat that commands and extensions address: the chat shown in the
    /// focused pane, else `current` while it is open, else a chat in view,
    /// else any open chat.
    pub fn active_chat(&self, current: Option<&str>) -> Option<String> {
        self.active(self.focused)
            .filter(|id| is_chat(id))
            .or_else(|| current.filter(|id| self.contains(id)))
            .or_else(|| self.visible().find(|id| is_chat(id)))
            .or_else(|| self.chats().next())
            .map(str::to_string)
    }
}
