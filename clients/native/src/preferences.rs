//! Client preferences stored on this device, independent of agent profiles.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::tabs::TabLayout;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    System,
    #[default]
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionPreference {
    #[default]
    Full,
    Reduced,
    Off,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendMode {
    #[default]
    Enter,
    CtrlEnter,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub theme: ThemePreference,
    pub motion: MotionPreference,
    pub send: SendMode,
    pub extensions: ExtensionPreferences,
    pub tabs: TabLayout,
}

/// Where the compact icon of an extension is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BadgePlace {
    Composer,
    Header,
    Hidden,
}

/// Which client extensions are on, and how their widgets are arranged.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExtensionPreferences {
    pub disabled: Vec<String>,
    pub panel_order: Vec<String>,
    pub collapsed: Vec<String>,
    /// Icon places chosen by the user; others use the extension default.
    pub badges: BTreeMap<String, BadgePlace>,
    pub badge_order: Vec<String>,
}

impl ExtensionPreferences {
    pub fn is_enabled(&self, id: &str) -> bool {
        !self.disabled.iter().any(|disabled| disabled == id)
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        self.disabled.retain(|disabled| disabled != id);
        if !enabled {
            self.disabled.push(id.to_string());
        }
    }

    pub fn is_collapsed(&self, id: &str) -> bool {
        self.collapsed.iter().any(|collapsed| collapsed == id)
    }

    pub fn toggle_collapsed(&mut self, id: &str) {
        if self.is_collapsed(id) {
            self.collapsed.retain(|collapsed| collapsed != id);
        } else {
            self.collapsed.push(id.to_string());
        }
    }

    /// Panel widgets in the saved order; ids the order does not mention yet
    /// follow in catalog order.
    pub fn ordered<'a>(&self, known: &[&'a str]) -> Vec<&'a str> {
        ordered(&self.panel_order, known)
    }

    /// Moves a panel widget in front of `before`, or to the end without it.
    pub fn move_before(&mut self, known: &[&str], id: &str, before: Option<&str>) {
        move_before(&mut self.panel_order, known, id, before);
    }

    pub fn badge_place(&self, id: &str, default: BadgePlace) -> BadgePlace {
        self.badges.get(id).copied().unwrap_or(default)
    }

    pub fn set_badge_place(&mut self, id: &str, place: BadgePlace) {
        self.badges.insert(id.to_string(), place);
    }

    pub fn badges_ordered<'a>(&self, known: &[&'a str]) -> Vec<&'a str> {
        ordered(&self.badge_order, known)
    }

    /// Puts an icon into `place`, in front of `before` or at its end.
    pub fn move_badge(
        &mut self,
        known: &[&str],
        id: &str,
        place: BadgePlace,
        before: Option<&str>,
    ) {
        self.set_badge_place(id, place);
        move_before(&mut self.badge_order, known, id, before);
    }
}

fn ordered<'a>(saved: &[String], known: &[&'a str]) -> Vec<&'a str> {
    let mut ordered: Vec<&'a str> = saved
        .iter()
        .filter_map(|id| known.iter().find(|known| *known == id).copied())
        .collect();
    for id in known {
        if !ordered.contains(id) {
            ordered.push(id);
        }
    }
    ordered
}

fn move_before(saved: &mut Vec<String>, known: &[&str], id: &str, before: Option<&str>) {
    if before == Some(id) {
        return;
    }
    let mut items = ordered(saved, known);
    let Some(from) = items.iter().position(|item| *item == id) else {
        return;
    };
    let item = items.remove(from);
    let to = before
        .and_then(|before| items.iter().position(|item| *item == before))
        .unwrap_or(items.len());
    items.insert(to, item);
    *saved = items.into_iter().map(str::to_string).collect();
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("dev.proteus.native").join("preferences.json"))
}

/// Saved preferences; a missing file means defaults.
pub fn load() -> Result<Preferences> {
    let Some(path) = path() else {
        return Ok(Preferences::default());
    };
    match fs::read_to_string(&path) {
        Ok(text) => {
            let preferences: Preferences = serde_json::from_str(&text)
                .with_context(|| format!("invalid preferences in {}", path.display()))?;
            preferences
                .tabs
                .validate()
                .map_err(|error| anyhow!("invalid tab layout in {}: {error}", path.display()))?;
            Ok(preferences)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Preferences::default()),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

pub fn save(preferences: &Preferences) -> Result<()> {
    let path = path().context("no HOME or XDG_CONFIG_HOME for preferences")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(preferences)?;
    fs::write(&path, text).with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{BadgePlace, ExtensionPreferences};

    const KNOWN: [&str; 3] = ["context", "plan", "notes"];

    #[test]
    fn widgets_reorder_and_new_ones_follow() {
        let mut extensions = ExtensionPreferences::default();
        assert_eq!(extensions.ordered(&KNOWN), KNOWN);

        extensions.move_before(&KNOWN, "notes", Some("context"));
        assert_eq!(extensions.ordered(&KNOWN), ["notes", "context", "plan"]);
        extensions.move_before(&KNOWN, "notes", None);
        assert_eq!(extensions.ordered(&KNOWN), KNOWN);

        extensions.move_before(&KNOWN, "context", Some("notes"));
        assert_eq!(extensions.ordered(&KNOWN), ["plan", "context", "notes"]);
        extensions.move_before(&KNOWN, "plan", Some("plan"));
        assert_eq!(extensions.ordered(&KNOWN), ["plan", "context", "notes"]);

        // An extension added later joins at the end; a removed one is skipped.
        assert_eq!(
            extensions.ordered(&["notes", "quota", "plan"]),
            ["plan", "notes", "quota"]
        );
    }

    #[test]
    fn switches_and_collapsing_toggle() {
        let mut extensions = ExtensionPreferences::default();
        extensions.set_enabled("files", false);
        assert!(!extensions.is_enabled("files"));
        extensions.set_enabled("files", true);
        assert!(extensions.is_enabled("files") && extensions.disabled.is_empty());
        extensions.toggle_collapsed("plan");
        assert!(extensions.is_collapsed("plan"));
        extensions.toggle_collapsed("plan");
        assert!(!extensions.is_collapsed("plan"));
    }

    #[test]
    fn badges_move_between_places() {
        let mut extensions = ExtensionPreferences::default();
        assert_eq!(
            extensions.badge_place("plan", BadgePlace::Composer),
            BadgePlace::Composer
        );
        extensions.move_badge(&KNOWN, "notes", BadgePlace::Header, Some("context"));
        assert_eq!(
            extensions.badge_place("notes", BadgePlace::Composer),
            BadgePlace::Header
        );
        assert_eq!(
            extensions.badges_ordered(&KNOWN),
            ["notes", "context", "plan"]
        );
        // The panel order is separate.
        assert_eq!(extensions.ordered(&KNOWN), KNOWN);
    }
}
