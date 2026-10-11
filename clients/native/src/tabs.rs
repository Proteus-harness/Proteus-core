//! Tab groups of the main area: which tabs are open, where they are, and how
//! two groups share the width. Pure state, persisted with the preferences.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// The conversation itself; always open, cannot be closed.
pub const CHAT: &str = "chat";
pub const MIN_SPLIT: u16 = 200;
pub const MAX_SPLIT: u16 = 800;
const EVEN_SPLIT: u16 = 500;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabLayout {
    pub groups: Vec<TabGroup>,
    pub focused: usize,
    /// Width share of the first group, in thousandths.
    pub split: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabGroup {
    pub tabs: Vec<String>,
    pub active: Option<String>,
    /// Shown again when the active tab closes.
    #[serde(skip)]
    pub previous: Option<String>,
}

impl Default for TabLayout {
    fn default() -> Self {
        Self {
            groups: vec![TabGroup {
                tabs: vec![CHAT.to_string()],
                active: Some(CHAT.to_string()),
                previous: None,
            }],
            focused: 0,
            split: EVEN_SPLIT,
        }
    }
}

impl TabLayout {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=2).contains(&self.groups.len()) {
            return Err("expected one or two tab groups".into());
        }
        if self.focused >= self.groups.len() {
            return Err("focused tab group does not exist".into());
        }
        if !(MIN_SPLIT..=MAX_SPLIT).contains(&self.split) {
            return Err(format!("split must be within {MIN_SPLIT}..={MAX_SPLIT}"));
        }
        let mut seen = HashSet::new();
        for group in &self.groups {
            for tab in &group.tabs {
                if !seen.insert(tab.as_str()) {
                    return Err(format!("tab `{tab}` is open twice"));
                }
            }
            match &group.active {
                Some(active) if !group.tabs.contains(active) => {
                    return Err(format!("active tab `{active}` is not open in its group"));
                }
                None if !group.tabs.is_empty() => {
                    return Err("a group with tabs has no active tab".into());
                }
                _ => {}
            }
        }
        if !seen.contains(CHAT) {
            return Err("the chat tab is missing".into());
        }
        Ok(())
    }

    pub fn is_split(&self) -> bool {
        self.groups.len() == 2
    }

    pub fn group_of(&self, id: &str) -> Option<usize> {
        self.groups
            .iter()
            .position(|group| group.tabs.iter().any(|tab| tab == id))
    }

    pub fn active(&self, group: usize) -> Option<&str> {
        self.groups.get(group)?.active.as_deref()
    }

    pub fn focus(&mut self, group: usize) {
        if group < self.groups.len() {
            self.focused = group;
        }
    }

    pub fn activate(&mut self, group: usize, id: &str) {
        let Some(tabs) = self.groups.get_mut(group) else {
            return;
        };
        if !tabs.tabs.iter().any(|tab| tab == id) {
            return;
        }
        if tabs.active.as_deref() != Some(id) {
            tabs.previous = tabs.active.replace(id.to_string());
        }
        self.focused = group;
    }

    /// Shows `id` where it is open, or opens it. A new tab avoids covering the
    /// chat: it goes to the other group, splitting the area when allowed.
    pub fn open(&mut self, id: &str, can_split: bool) {
        if let Some(group) = self.group_of(id) {
            self.activate(group, id);
            return;
        }
        let mut target = self.focused;
        if self.active(target) == Some(CHAT) {
            if self.is_split() {
                target = 1 - target;
            } else if can_split {
                self.groups.push(TabGroup::default());
                target = 1;
            }
        }
        self.groups[target].tabs.push(id.to_string());
        self.activate(target, id);
    }

    /// Opens `id` in `group`, moving it there when open elsewhere.
    pub fn open_in(&mut self, id: &str, group: usize) {
        if group >= self.groups.len() {
            return;
        }
        match self.group_of(id) {
            Some(from) if from != group => self.move_tab(id, group, None),
            Some(_) => self.activate(group, id),
            None => {
                self.groups[group].tabs.push(id.to_string());
                self.activate(group, id);
            }
        }
    }

    pub fn close(&mut self, id: &str) {
        if id == CHAT {
            return;
        }
        let Some(group) = self.group_of(id) else {
            return;
        };
        self.detach(group, id);
        self.drop_if_empty(group);
    }

    /// Moves a tab in front of `before` in `group`, or to the end of it.
    /// `group == groups.len()` puts it into a new second group.
    pub fn move_tab(&mut self, id: &str, group: usize, before: Option<&str>) {
        let Some(from) = self.group_of(id) else {
            return;
        };
        if group == from {
            let tabs = &mut self.groups[group].tabs;
            if before != Some(id) {
                let ix = tabs.iter().position(|tab| tab == id).expect("tab is open");
                let tab = tabs.remove(ix);
                let at = before
                    .and_then(|before| tabs.iter().position(|tab| tab == before))
                    .unwrap_or(tabs.len());
                tabs.insert(at, tab);
            }
            self.activate(group, id);
            return;
        }
        if group > self.groups.len() || (group == self.groups.len() && self.is_split()) {
            return;
        }
        if group == self.groups.len() {
            if self.groups[from].tabs.len() == 1 {
                return;
            }
            self.groups.push(TabGroup::default());
        }
        self.detach(from, id);
        let target = &mut self.groups[group];
        let at = before
            .and_then(|before| target.tabs.iter().position(|tab| tab == before))
            .unwrap_or(target.tabs.len());
        target.tabs.insert(at, id.to_string());
        self.activate(group, id);
        self.drop_if_empty(from);
    }

    /// Moves the active tab of `group` to the other group, opening one if
    /// needed.
    pub fn transfer(&mut self, group: usize) {
        let Some(id) = self.active(group).map(str::to_string) else {
            return;
        };
        let target = if self.is_split() { 1 - group } else { 1 };
        self.move_tab(&id, target, None);
    }

    /// Merges two groups, or opens an empty second one.
    pub fn toggle_split(&mut self) {
        if self.is_split() {
            let second = self.groups.remove(1);
            let first = &mut self.groups[0];
            first.tabs.extend(second.tabs);
            if self.focused == 1
                && let Some(active) = second.active
            {
                first.previous = first.active.replace(active);
            }
            if first.active.is_none() {
                first.active = first.tabs.first().cloned();
            }
            self.focused = 0;
        } else {
            self.groups.push(TabGroup::default());
            self.focused = 1;
        }
    }

    /// Cycles the focused group's tabs; returns the newly active one.
    pub fn step(&mut self, offset: isize) -> Option<String> {
        let group = &self.groups[self.focused];
        if group.tabs.is_empty() {
            return None;
        }
        let current = group
            .active
            .as_ref()
            .and_then(|active| group.tabs.iter().position(|tab| tab == active))
            .unwrap_or(0);
        let next = (current as isize + offset).rem_euclid(group.tabs.len() as isize) as usize;
        let id = group.tabs[next].clone();
        self.activate(self.focused, &id);
        Some(id)
    }

    pub fn set_split(&mut self, split: u16) {
        self.split = split.clamp(MIN_SPLIT, MAX_SPLIT);
    }

    pub fn reset_split(&mut self) {
        self.split = EVEN_SPLIT;
    }

    /// Closes tabs whose pages are gone, e.g. of a switched-off extension.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        for group in 0..self.groups.len() {
            let gone: Vec<String> = self.groups[group]
                .tabs
                .iter()
                .filter(|tab| tab.as_str() != CHAT && !keep(tab))
                .cloned()
                .collect();
            for id in gone {
                self.detach(group, &id);
            }
        }
    }

    fn detach(&mut self, group: usize, id: &str) {
        let tabs = &mut self.groups[group];
        let Some(ix) = tabs.tabs.iter().position(|tab| tab == id) else {
            return;
        };
        tabs.tabs.remove(ix);
        if tabs.previous.as_deref() == Some(id) {
            tabs.previous = None;
        }
        if tabs.active.as_deref() == Some(id) {
            tabs.active = tabs
                .previous
                .take()
                .or_else(|| tabs.tabs.get(ix.saturating_sub(1)).cloned());
        }
    }

    fn drop_if_empty(&mut self, group: usize) {
        if self.is_split() && self.groups[group].tabs.is_empty() {
            self.groups.remove(group);
            self.focused = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CHAT, TabLayout};

    fn tabs(layout: &TabLayout) -> Vec<(Vec<&str>, Option<&str>)> {
        layout
            .groups
            .iter()
            .map(|group| {
                (
                    group.tabs.iter().map(String::as_str).collect(),
                    group.active.as_deref(),
                )
            })
            .collect()
    }

    #[test]
    fn opening_avoids_covering_the_chat() {
        let mut layout = TabLayout::default();
        layout.open("files", false);
        assert_eq!(tabs(&layout), [(vec![CHAT, "files"], Some("files"))]);

        let mut layout = TabLayout::default();
        layout.open("files", true);
        assert_eq!(
            tabs(&layout),
            [(vec![CHAT], Some(CHAT)), (vec!["files"], Some("files"))]
        );
        assert_eq!(layout.focused, 1);

        // Opening an open tab only shows it; the chat group stays as it is.
        layout.focus(0);
        layout.open("notes", true);
        layout.open("files", true);
        assert_eq!(layout.active(1), Some("files"));
        assert_eq!(layout.groups[1].tabs, ["files", "notes"]);
        assert!(layout.validate().is_ok());
    }

    #[test]
    fn closing_falls_back_and_merges_empty_groups() {
        let mut layout = TabLayout::default();
        layout.open("files", false);
        layout.open("notes", false);
        layout.activate(0, "files");
        layout.activate(0, "notes");
        layout.close("notes");
        assert_eq!(layout.active(0), Some("files"));
        layout.close(CHAT);
        assert_eq!(layout.groups[0].tabs, [CHAT, "files"]);

        layout.transfer(0);
        assert_eq!(
            tabs(&layout),
            [(vec![CHAT], Some(CHAT)), (vec!["files"], Some("files"))]
        );
        layout.close("files");
        assert_eq!(tabs(&layout), [(vec![CHAT], Some(CHAT))]);
        assert_eq!(layout.focused, 0);
    }

    #[test]
    fn moving_reorders_and_splits() {
        let mut layout = TabLayout::default();
        layout.open("files", false);
        layout.open("notes", false);
        layout.move_tab("notes", 0, Some(CHAT));
        assert_eq!(layout.groups[0].tabs, ["notes", CHAT, "files"]);

        layout.move_tab(CHAT, 1, None);
        assert_eq!(
            tabs(&layout),
            [
                (vec!["notes", "files"], Some("notes")),
                (vec![CHAT], Some(CHAT))
            ]
        );
        // A third group is not possible; dropping the last tab merges back.
        layout.move_tab("files", 2, None);
        assert_eq!(layout.groups.len(), 2);
        layout.move_tab(CHAT, 0, Some("files"));
        assert_eq!(tabs(&layout), [(vec!["notes", CHAT, "files"], Some(CHAT))]);
    }

    #[test]
    fn split_toggles_and_validation_rejects_broken_layouts() {
        let mut layout = TabLayout::default();
        layout.open("files", false);
        layout.toggle_split();
        assert_eq!(tabs(&layout)[1], (vec![], None));
        assert!(layout.validate().is_ok());
        layout.toggle_split();
        assert_eq!(tabs(&layout), [(vec![CHAT, "files"], Some("files"))]);

        layout.retain(|id| id != "files");
        assert_eq!(tabs(&layout), [(vec![CHAT], Some(CHAT))]);

        let mut broken = TabLayout::default();
        broken.groups[0].tabs.push(CHAT.into());
        assert!(broken.validate().is_err());
        let mut broken = TabLayout::default();
        broken.groups[0].tabs = vec!["files".into()];
        broken.groups[0].active = Some("files".into());
        assert!(broken.validate().is_err());
    }
}
