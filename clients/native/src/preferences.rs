//! Client preferences stored on this device, independent of agent profiles.

use std::collections::BTreeMap;
use std::fs;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

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

/// What the hover tooltip of a control says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hints {
    /// What the control does and its shortcut.
    #[default]
    Full,
    Description,
    /// The shortcut alone; a control without one has no tooltip.
    Keys,
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
    pub hints: Hints,
    pub send: SendMode,
    pub notifications: Notifications,
    pub view: ViewPreferences,
    pub extensions: ExtensionPreferences,
    pub catalogs: crate::catalog::Preferences,
    pub sidebar: SidebarPreferences,
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

/// Panel of the session list; other panels are named by their extension.
pub const SESSIONS_PANEL: &str = "sessions";
/// Width of the side panel, px at 100%.
pub const SIDEBAR_WIDTH: RangeInclusive<f32> = 180.0..=640.0;

/// The side panel between the rail and the tabs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SidebarPreferences {
    /// Panel shown: [`SESSIONS_PANEL`] or an extension id; none while the
    /// side panel is closed.
    pub panel: Option<String>,
    pub width: f32,
}

impl Default for SidebarPreferences {
    fn default() -> Self {
        Self {
            panel: Some(SESSIONS_PANEL.to_owned()),
            width: 264.,
        }
    }
}

impl SidebarPreferences {
    fn validate(&self) -> Result<()> {
        if !SIDEBAR_WIDTH.contains(&self.width) {
            return Err(anyhow!("width {} is outside {SIDEBAR_WIDTH:?}", self.width));
        }
        Ok(())
    }
}

/// Where the compact icon of an extension is shown: the left or the right
/// end of the composer row and the top bar, or the top or the bottom of the
/// rail along the left edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BadgePlace {
    ComposerStart,
    ComposerEnd,
    HeaderStart,
    HeaderEnd,
    RailStart,
    RailEnd,
    Hidden,
}

impl BadgePlace {
    /// Every place where icons are drawn.
    pub const SHOWN: [BadgePlace; 6] = [
        BadgePlace::ComposerStart,
        BadgePlace::ComposerEnd,
        BadgePlace::HeaderStart,
        BadgePlace::HeaderEnd,
        BadgePlace::RailStart,
        BadgePlace::RailEnd,
    ];

    /// Icons here stand in a column, so only narrow ones fit.
    pub fn is_rail(self) -> bool {
        matches!(self, BadgePlace::RailStart | BadgePlace::RailEnd)
    }
}

/// Which client extensions are on, and where their icons are.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExtensionPreferences {
    /// Extension order in the settings list and their pages.
    pub order: Vec<String>,
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

    /// Extensions in the saved order; ids it does not mention yet follow in
    /// catalog order.
    pub fn ordered<'a>(&self, known: &[&'a str]) -> Vec<&'a str> {
        ordered(&self.order, known)
    }

    /// Moves an extension in front of `before` or to the end of the list.
    pub fn move_extension(&mut self, known: &[&str], id: &str, before: Option<&str>) {
        move_before(&mut self.order, known, id, before);
    }

    /// Drops everything remembered about a removed extension.
    pub fn forget(&mut self, id: &str) {
        for list in [
            &mut self.order,
            &mut self.disabled,
            &mut self.badge_order,
            &mut self.pinned,
        ] {
            list.retain(|item| item != id);
        }
        self.badges.remove(id);
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

/// Saved preferences; a missing file means defaults. A file that cannot be
/// used is moved aside, so the next save does not overwrite it.
pub fn load() -> Result<Preferences> {
    let Some(path) = path() else {
        return Ok(Preferences::default());
    };
    match fs::read_to_string(&path) {
        Ok(text) => parse(&text, &path).map_err(|error| match set_aside(&path) {
            Ok(kept) => error.context(format!("the file was moved to {}", kept.display())),
            Err(_) => error,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Preferences::default()),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn parse(text: &str, path: &Path) -> Result<Preferences> {
    let preferences: Preferences = serde_json::from_str(text)
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
        .sidebar
        .validate()
        .with_context(|| format!("invalid side panel in {}", path.display()))?;
    preferences
        .shortcuts
        .validate()
        .with_context(|| format!("invalid shortcuts in {}", path.display()))?;
    Ok(preferences)
}

fn set_aside(path: &Path) -> std::io::Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let kept = path.with_extension(format!("json.broken-{stamp}"));
    fs::rename(path, &kept)?;
    Ok(kept)
}

/// Writes a sibling file first, so a crash never leaves half a file.
pub fn save(preferences: &Preferences) -> Result<()> {
    let path = path().context("no HOME or XDG_CONFIG_HOME for preferences")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(preferences)?;
    let draft = path.with_extension("json.tmp");
    fs::write(&draft, text).with_context(|| format!("failed to write {}", draft.display()))?;
    fs::rename(&draft, &path).with_context(|| format!("failed to write {}", path.display()))
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
            extensions.badge_place("plan", BadgePlace::ComposerStart),
            BadgePlace::ComposerStart
        );
        extensions.move_badge(&KNOWN, "files", BadgePlace::HeaderEnd, Some("context"));
        assert_eq!(
            extensions.badge_place("files", BadgePlace::ComposerStart),
            BadgePlace::HeaderEnd
        );
        assert_eq!(
            extensions.badges_ordered(&KNOWN),
            ["files", "context", "plan"]
        );
        // Dropping an icon onto itself keeps the order.
        extensions.move_badge(&KNOWN, "plan", BadgePlace::ComposerStart, Some("plan"));
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
    fn extensions_reorder_and_are_forgotten_on_removal() {
        let mut extensions = ExtensionPreferences::default();
        extensions.move_extension(&KNOWN, "files", Some("context"));
        assert_eq!(extensions.ordered(&KNOWN), ["files", "context", "plan"]);
        extensions.set_enabled("files", false);
        extensions.move_badge(&KNOWN, "files", BadgePlace::HeaderEnd, None);
        extensions.set_pinned("files", true);
        extensions.forget("files");
        assert_eq!(extensions.ordered(&KNOWN), ["context", "plan", "files"]);
        assert!(extensions.is_enabled("files") && !extensions.is_pinned("files"));
        assert!(extensions.badges.is_empty() && extensions.badge_order.len() == 2);
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
