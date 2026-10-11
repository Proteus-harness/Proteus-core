//! Tab groups of the main area: which tabs are open, where they are, and how
//! two groups share the width. Pure state, persisted with the preferences.
//!
//! Tabs are chats, addressed as `chat:<session dir>`, and documents of
//! extensions, addressed as `<extension>:<argument>`, e.g. `files:src/main.rs`.
//! No extension is called `chat`, so the two never mix.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Prefix of chat tab ids.
const CHAT_PREFIX: &str = "chat:";
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
    /// Tab that the next previewed document replaces, so browsing does not
    /// pile up tabs. Keeping or moving it makes it a regular tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// Tab id of an extension document.
pub fn document(extension: &str, argument: &str) -> String {
    format!("{extension}:{argument}")
}

/// Extension and argument of a document tab; `None` for chats.
pub fn split_document(id: &str) -> Option<(&str, &str)> {
    if is_chat(id) {
        return None;
    }
    id.split_once(':')
}

/// Tab id of the chat of an absolute session directory.
pub fn chat(session_dir: &Path) -> String {
    format!("{CHAT_PREFIX}{}", session_dir.to_string_lossy())
}

/// Session directory of a chat tab; `None` for documents.
pub fn chat_dir(id: &str) -> Option<&Path> {
    id.strip_prefix(CHAT_PREFIX).map(Path::new)
}

pub fn is_chat(id: &str) -> bool {
    id.starts_with(CHAT_PREFIX)
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
            groups: vec![TabGroup::default()],
            focused: 0,
            split: EVEN_SPLIT,
            preview: None,
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
        if let Some(preview) = &self.preview
            && !seen.contains(preview.as_str())
        {
            return Err(format!("preview tab `{preview}` is not open"));
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

    /// Shows `id` where it is open, or opens it. A chat joins the chat in
    /// view; a document avoids covering a chat: it goes to the other group,
    /// splitting the area when allowed.
    pub fn open(&mut self, id: &str, can_split: bool) {
        if let Some(group) = self.group_of(id) {
            self.activate(group, id);
            return;
        }
        let mut target = self.focused;
        let shows_chat = |group| self.active(group).is_some_and(is_chat);
        if is_chat(id) {
            if self.is_split() && !shows_chat(target) && shows_chat(1 - target) {
                target = 1 - target;
            }
        } else if shows_chat(target) {
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

    /// Opens `id` in place of the preview tab, or like [`Self::open`] when
    /// there is none; `id` becomes the preview. An open tab just comes
    /// forward and keeps its kind.
    pub fn open_preview(&mut self, id: &str, can_split: bool) {
        if let Some(group) = self.group_of(id) {
            self.activate(group, id);
            return;
        }
        let replaced = self
            .preview
            .take()
            .and_then(|old| Some((self.group_of(&old)?, old)));
        match replaced {
            Some((group, old)) => {
                let tabs = &mut self.groups[group];
                let ix = tabs
                    .tabs
                    .iter()
                    .position(|tab| *tab == old)
                    .expect("tab is open");
                tabs.tabs[ix] = id.to_string();
                if tabs.previous.as_deref() == Some(old.as_str()) {
                    tabs.previous = None;
                }
                if tabs.active.as_deref() == Some(old.as_str()) {
                    tabs.active = Some(id.to_string());
                    self.focused = group;
                } else {
                    self.activate(group, id);
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
        self.keep(id);
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

    /// Closes tabs whose pages are gone, e.g. of a switched-off extension or
    /// a deleted session.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        for group in 0..self.groups.len() {
            let gone: Vec<String> = self.groups[group]
                .tabs
                .iter()
                .filter(|tab| !keep(tab))
                .cloned()
                .collect();
            for id in gone {
                self.detach(group, &id);
            }
        }
    }

    /// Open chat tabs, group by group.
    pub fn chats(&self) -> impl Iterator<Item = &str> {
        self.groups
            .iter()
            .flat_map(|group| group.tabs.iter())
            .map(String::as_str)
            .filter(|id| is_chat(id))
    }

    /// The chat that commands and extensions address: the chat shown in the
    /// focused group, else `current` while it is open, else a chat in view,
    /// else any open chat.
    pub fn active_chat(&self, current: Option<&str>) -> Option<String> {
        let shown = |group: usize| self.active(group).filter(|id| is_chat(id));
        shown(self.focused)
            .or_else(|| current.filter(|id| self.group_of(id).is_some()))
            .or_else(|| (0..self.groups.len()).find_map(shown))
            .or_else(|| self.chats().next())
            .map(str::to_string)
    }

    fn detach(&mut self, group: usize, id: &str) {
        self.keep(id);
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
mod tests;
