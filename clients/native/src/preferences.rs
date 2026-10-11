//! Client preferences stored on this device, independent of agent profiles.

use std::collections::BTreeMap;
use std::fs;
use std::ops::RangeInclusive;
use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::recent::RecentWorkspaces;
use crate::sessions::SessionMarks;
use crate::shortcuts::Shortcuts;
use crate::tabs::TabLayout;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    System,
    #[default]
    Dark,
    Light,
}

pub const DEFAULT_DARK_THEME: &str = "Proteus Graphite";
pub const DEFAULT_LIGHT_THEME: &str = "Proteus Light";

/// Colour theme for each appearance, by theme name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeNames {
    pub dark: String,
    pub light: String,
}

impl Default for ThemeNames {
    fn default() -> Self {
        Self {
            dark: DEFAULT_DARK_THEME.to_owned(),
            light: DEFAULT_LIGHT_THEME.to_owned(),
        }
    }
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

/// System notifications about chats while the window is in the background.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notifications {
    #[default]
    Background,
    Off,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub theme: ThemePreference,
    pub themes: ThemeNames,
    pub motion: MotionPreference,
    pub send: SendMode,
    pub notifications: Notifications,
    pub view: ViewPreferences,
    pub extensions: ExtensionPreferences,
    pub tabs: TabLayout,
    pub sessions: SessionMarks,
    pub recent: RecentWorkspaces,
    pub shortcuts: Shortcuts,
}

/// Interface scale steps in percent, for the menu and Ctrl+=/Ctrl+-.
pub const SCALES: [u16; 9] = [75, 80, 90, 100, 110, 125, 150, 175, 200];
/// Text size of chat messages and the composer, px at 100%.
pub const CHAT_TEXT: RangeInclusive<u16> = 12..=22;
/// Widest conversation column, px at 100%.
pub const CHAT_WIDTH: RangeInclusive<u16> = 420..=1600;

/// Sizes of the interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ViewPreferences {
    /// Interface scale in percent, one of [`SCALES`].
    pub scale: u16,
    pub chat_text: u16,
    pub chat_width: u16,
    /// Fold runs of tool calls into one row.
    pub compact_tools: bool,
    /// Follow a streaming answer to the end of the chat.
    pub autoscroll: bool,
}

impl Default for ViewPreferences {
    fn default() -> Self {
        Self {
            scale: 100,
            chat_text: 16,
            chat_width: 820,
            compact_tools: true,
            autoscroll: true,
        }
    }
}

impl ViewPreferences {
    pub fn scale_factor(&self) -> f32 {
        f32::from(self.scale) / 100.
    }

    /// Moves the scale `steps` positions along [`SCALES`]; 0 resets it.
    pub fn zoom(&mut self, steps: i32) {
        if steps == 0 {
            self.scale = Self::default().scale;
            return;
        }
        let at = SCALES
            .iter()
            .position(|scale| *scale >= self.scale)
            .unwrap_or(SCALES.len() - 1) as i32;
        let to = (at + steps).clamp(0, SCALES.len() as i32 - 1);
        self.scale = SCALES[to as usize];
    }

    fn validate(&self) -> Result<()> {
        if !SCALES.contains(&self.scale) {
            return Err(anyhow!("scale {} is not one of {SCALES:?}", self.scale));
        }
        if !CHAT_TEXT.contains(&self.chat_text) {
            return Err(anyhow!(
                "chat_text {} is outside {CHAT_TEXT:?}",
                self.chat_text
            ));
        }
        if !CHAT_WIDTH.contains(&self.chat_width) {
            return Err(anyhow!(
                "chat_width {} is outside {CHAT_WIDTH:?}",
                self.chat_width
            ));
        }
        Ok(())
    }
}

/// Where the compact icon of an extension is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BadgePlace {
    Composer,
    Header,
    Hidden,
}

/// Which client extensions are on, and where their icons are.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExtensionPreferences {
    pub disabled: Vec<String>,
    /// Icon places chosen by the user; others use the extension default.
    pub badges: BTreeMap<String, BadgePlace>,
    pub badge_order: Vec<String>,
    /// Icons fixed in place: they cannot be dragged.
    pub pinned: Vec<String>,
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

    pub fn badge_place(&self, id: &str, default: BadgePlace) -> BadgePlace {
        self.badges.get(id).copied().unwrap_or(default)
    }

    pub fn set_badge_place(&mut self, id: &str, place: BadgePlace) {
        self.badges.insert(id.to_string(), place);
    }

    /// Icons in the saved order; ids the order does not mention yet follow
    /// in catalog order.
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

    pub fn is_pinned(&self, id: &str) -> bool {
        self.pinned.iter().any(|pinned| pinned == id)
    }

    pub fn set_pinned(&mut self, id: &str, pinned: bool) {
        self.pinned.retain(|item| item != id);
        if pinned {
            self.pinned.push(id.to_string());
        }
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

/// Directory of the client's files on this device.
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join(crate::APP_ID))
}

fn path() -> Option<PathBuf> {
    Some(config_dir()?.join("preferences.json"))
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
            preferences
                .view
                .validate()
                .with_context(|| format!("invalid view sizes in {}", path.display()))?;
            preferences
                .shortcuts
                .validate()
                .with_context(|| format!("invalid shortcuts in {}", path.display()))?;
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
    use super::{BadgePlace, ExtensionPreferences, ViewPreferences};

    const KNOWN: [&str; 3] = ["context", "plan", "files"];

    #[test]
    fn switches_and_pins_toggle() {
        let mut extensions = ExtensionPreferences::default();
        extensions.set_enabled("files", false);
        assert!(!extensions.is_enabled("files"));
        extensions.set_enabled("files", true);
        assert!(extensions.is_enabled("files") && extensions.disabled.is_empty());
        extensions.set_pinned("plan", true);
        extensions.set_pinned("plan", true);
        assert!(extensions.is_pinned("plan") && extensions.pinned.len() == 1);
        extensions.set_pinned("plan", false);
        assert!(!extensions.is_pinned("plan"));
    }

    #[test]
    fn badges_move_between_places() {
        let mut extensions = ExtensionPreferences::default();
        assert_eq!(extensions.badges_ordered(&KNOWN), KNOWN);
        assert_eq!(
            extensions.badge_place("plan", BadgePlace::Composer),
            BadgePlace::Composer
        );
        extensions.move_badge(&KNOWN, "files", BadgePlace::Header, Some("context"));
        assert_eq!(
            extensions.badge_place("files", BadgePlace::Composer),
            BadgePlace::Header
        );
        assert_eq!(
            extensions.badges_ordered(&KNOWN),
            ["files", "context", "plan"]
        );
        // Dropping an icon onto itself keeps the order.
        extensions.move_badge(&KNOWN, "plan", BadgePlace::Composer, Some("plan"));
        assert_eq!(
            extensions.badges_ordered(&KNOWN),
            ["files", "context", "plan"]
        );
        // An extension added later joins at the end; a removed one is skipped.
        assert_eq!(
            extensions.badges_ordered(&["plan", "quota", "files"]),
            ["files", "plan", "quota"]
        );
    }

    #[test]
    fn zoom_walks_the_scale_steps() {
        let mut view = ViewPreferences::default();
        view.zoom(1);
        assert_eq!(view.scale, 110);
        view.zoom(-3);
        assert_eq!(view.scale, 80);
        view.zoom(-5);
        assert_eq!(view.scale, 75);
        view.zoom(20);
        assert_eq!(view.scale, 200);
        view.zoom(0);
        assert_eq!(view.scale, 100);
        assert!(view.validate().is_ok());
        view.chat_width = 100;
        assert!(view.validate().is_err());
    }
}
