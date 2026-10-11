//! Keyboard shortcuts of the window's commands: defaults plus the user's
//! overrides, kept headless so the keymap is rebuilt from data.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Command {
    NewSession,
    NewTerminal,
    ToggleSettings,
    ToggleSessions,
    ToggleFiles,
    NextTab,
    PreviousTab,
    SplitRight,
    SplitDown,
    FocusPaneLeft,
    FocusPaneRight,
    FocusPaneUp,
    FocusPaneDown,
    ZoomIn,
    ZoomOut,
    ZoomReset,
}

impl Command {
    pub const ALL: [Command; 16] = [
        Command::NewSession,
        Command::NewTerminal,
        Command::ToggleSettings,
        Command::ToggleSessions,
        Command::ToggleFiles,
        Command::NextTab,
        Command::PreviousTab,
        Command::SplitRight,
        Command::SplitDown,
        Command::FocusPaneLeft,
        Command::FocusPaneRight,
        Command::FocusPaneUp,
        Command::FocusPaneDown,
        Command::ZoomIn,
        Command::ZoomOut,
        Command::ZoomReset,
    ];

    /// Stable key in preferences.
    pub fn id(self) -> &'static str {
        match self {
            Command::NewSession => "new_session",
            Command::NewTerminal => "new_terminal",
            Command::ToggleSettings => "toggle_settings",
            Command::ToggleSessions => "toggle_sessions",
            Command::ToggleFiles => "toggle_files",
            Command::NextTab => "next_tab",
            Command::PreviousTab => "previous_tab",
            Command::SplitRight => "split_right",
            Command::SplitDown => "split_down",
            Command::FocusPaneLeft => "focus_pane_left",
            Command::FocusPaneRight => "focus_pane_right",
            Command::FocusPaneUp => "focus_pane_up",
            Command::FocusPaneDown => "focus_pane_down",
            Command::ZoomIn => "zoom_in",
            Command::ZoomOut => "zoom_out",
            Command::ZoomReset => "zoom_reset",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Command::NewSession => "Новая сессия",
            Command::NewTerminal => "Новый терминал",
            Command::ToggleSettings => "Настройки",
            Command::ToggleSessions => "Панель сессий",
            Command::ToggleFiles => "Панель файлов",
            Command::NextTab => "Следующая вкладка",
            Command::PreviousTab => "Предыдущая вкладка",
            Command::SplitRight => "Отделить вкладку вправо",
            Command::SplitDown => "Отделить вкладку вниз",
            Command::FocusPaneLeft => "Область слева",
            Command::FocusPaneRight => "Область справа",
            Command::FocusPaneUp => "Область сверху",
            Command::FocusPaneDown => "Область снизу",
            Command::ZoomIn => "Увеличить масштаб",
            Command::ZoomOut => "Уменьшить масштаб",
            Command::ZoomReset => "Сбросить масштаб",
        }
    }

    pub fn defaults(self) -> &'static [&'static str] {
        match self {
            Command::NewSession => &["ctrl-n"],
            Command::NewTerminal => &["ctrl-`"],
            Command::ToggleSettings => &["ctrl-,"],
            Command::ToggleSessions => &["ctrl-shift-h"],
            Command::ToggleFiles => &["ctrl-shift-e"],
            Command::NextTab => &["ctrl-tab"],
            Command::PreviousTab => &["ctrl-shift-tab"],
            Command::SplitRight => &["ctrl-\\"],
            // Shift+\ arrives as `|`.
            Command::SplitDown => &["ctrl-|"],
            Command::FocusPaneLeft => &["ctrl-alt-left"],
            Command::FocusPaneRight => &["ctrl-alt-right"],
            Command::FocusPaneUp => &["ctrl-alt-up"],
            Command::FocusPaneDown => &["ctrl-alt-down"],
            // Shift+= arrives as `+`.
            Command::ZoomIn => &["ctrl-=", "ctrl-+"],
            Command::ZoomOut => &["ctrl--"],
            Command::ZoomReset => &["ctrl-0"],
        }
    }

    /// Whether the shortcut wins over a focused terminal; otherwise the key
    /// goes to the shell (Ctrl+N is shell history there).
    pub fn in_terminal(self) -> bool {
        self != Command::NewSession
    }

    fn from_id(id: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|command| command.id() == id)
    }
}

/// The user's overrides by command id; an empty list disables the command.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Shortcuts(BTreeMap<String, Vec<String>>);

impl Shortcuts {
    pub fn chords(&self, command: Command) -> Vec<String> {
        match self.0.get(command.id()) {
            Some(chords) => chords.clone(),
            None => command
                .defaults()
                .iter()
                .map(|chord| chord.to_string())
                .collect(),
        }
    }

    pub fn is_default(&self, command: Command) -> bool {
        !self.0.contains_key(command.id())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Adds a chord to the command and takes it from every other command.
    pub fn add(&mut self, command: Command, chord: &str) -> Result<()> {
        let chord = normalize(chord)?;
        for other in Command::ALL {
            if other != command && self.chords(other).contains(&chord) {
                let rest = self
                    .chords(other)
                    .into_iter()
                    .filter(|it| *it != chord)
                    .collect();
                self.set(other, rest);
            }
        }
        let mut chords = self.chords(command);
        if !chords.contains(&chord) {
            chords.push(chord);
        }
        self.set(command, chords);
        Ok(())
    }

    pub fn remove(&mut self, command: Command, chord: &str) {
        let chords = self
            .chords(command)
            .into_iter()
            .filter(|it| it != chord)
            .collect();
        self.set(command, chords);
    }

    pub fn reset(&mut self, command: Command) {
        self.0.remove(command.id());
    }

    pub fn reset_all(&mut self) {
        self.0.clear();
    }

    /// Chords bound to more than one command.
    fn conflicts(&self) -> BTreeMap<String, Vec<Command>> {
        let mut owners: BTreeMap<String, Vec<Command>> = BTreeMap::new();
        for command in Command::ALL {
            for chord in self.chords(command) {
                owners.entry(chord).or_default().push(command);
            }
        }
        owners.retain(|_, commands| commands.len() > 1);
        owners
    }

    pub fn validate(&self) -> Result<()> {
        for (id, chords) in &self.0 {
            if Command::from_id(id).is_none() {
                bail!("unknown shortcut command `{id}`");
            }
            for chord in chords {
                if normalize(chord)? != *chord {
                    bail!("shortcut `{chord}` of `{id}` is not in canonical form");
                }
            }
        }
        if let Some((chord, commands)) = self.conflicts().into_iter().next() {
            let ids: Vec<&str> = commands.iter().map(|command| command.id()).collect();
            bail!(
                "shortcut `{chord}` is bound to several commands: {}",
                ids.join(", ")
            );
        }
        Ok(())
    }

    fn set(&mut self, command: Command, chords: Vec<String>) {
        let defaults: Vec<String> = command.defaults().iter().map(|it| it.to_string()).collect();
        if chords == defaults {
            self.0.remove(command.id());
        } else {
            self.0.insert(command.id().to_string(), chords);
        }
    }
}

const MODIFIERS: [&str; 5] = ["ctrl", "alt", "shift", "super", "fn"];

/// Canonical chord text: lower case, modifiers in a fixed order, one key.
pub fn normalize(chord: &str) -> Result<String> {
    let chord = chord.trim().to_lowercase();
    // A trailing `-` is the minus key itself (`ctrl--`).
    let (head, key) = match chord.strip_suffix("--") {
        Some(head) => (head, "-".to_string()),
        None if chord == "-" => ("", "-".to_string()),
        None => match chord.rsplit_once('-') {
            Some((head, key)) => (head, key.to_string()),
            None => ("", chord.clone()),
        },
    };
    if key.is_empty() || key.contains(' ') {
        bail!("shortcut `{chord}` has no key");
    }
    if MODIFIERS.contains(&key.as_str()) {
        bail!("shortcut `{chord}` has only modifiers");
    }
    let mut present = [false; MODIFIERS.len()];
    if !head.is_empty() {
        for part in head.split('-') {
            let Some(index) = MODIFIERS.iter().position(|it| *it == part) else {
                bail!("unknown modifier `{part}` in shortcut `{chord}`");
            };
            present[index] = true;
        }
    }
    let mut parts: Vec<&str> = MODIFIERS
        .iter()
        .zip(present)
        .filter(|(_, on)| *on)
        .map(|(name, _)| *name)
        .collect();
    parts.push(&key);
    Ok(parts.join("-"))
}

/// Human form of a chord: `Ctrl+Shift+E`.
pub fn label(chord: &str) -> String {
    let (head, key) = match chord.strip_suffix("--") {
        Some(head) => (head, "-"),
        None => chord.rsplit_once('-').unwrap_or(("", chord)),
    };
    let mut parts: Vec<String> = head
        .split('-')
        .filter(|part| !part.is_empty())
        .map(|part| match part {
            "ctrl" => "Ctrl".to_string(),
            "alt" => "Alt".to_string(),
            "shift" => "Shift".to_string(),
            "super" => "Super".to_string(),
            other => other.to_string(),
        })
        .collect();
    let key = match key {
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "tab" => "Tab".to_string(),
        "enter" => "Enter".to_string(),
        "space" => "Space".to_string(),
        "escape" => "Esc".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" => "Delete".to_string(),
        other if other.chars().count() == 1 => other.to_uppercase(),
        other => {
            let mut chars = other.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    };
    parts.push(key);
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_chords() {
        assert_eq!(normalize("Shift-Ctrl-E").unwrap(), "ctrl-shift-e");
        assert_eq!(normalize("ctrl--").unwrap(), "ctrl--");
        assert_eq!(normalize("ctrl-alt-left").unwrap(), "ctrl-alt-left");
        assert_eq!(normalize("f5").unwrap(), "f5");
        assert!(normalize("ctrl-shift").is_err());
        assert!(normalize("hyper-x").is_err());
        assert!(normalize("").is_err());
        assert_eq!(label("ctrl-shift-e"), "Ctrl+Shift+E");
        assert_eq!(label("ctrl--"), "Ctrl+-");
        assert_eq!(label("ctrl-alt-left"), "Ctrl+Alt+←");
    }

    #[test]
    fn overrides_move_chords_between_commands() {
        let mut shortcuts = Shortcuts::default();
        assert!(shortcuts.conflicts().is_empty());
        shortcuts.add(Command::NewTerminal, "Ctrl-N").unwrap();
        assert_eq!(shortcuts.chords(Command::NewTerminal), ["ctrl-`", "ctrl-n"]);
        assert!(shortcuts.chords(Command::NewSession).is_empty());
        assert!(!shortcuts.is_default(Command::NewSession));
        assert!(shortcuts.conflicts().is_empty());
        shortcuts.validate().unwrap();

        shortcuts.remove(Command::NewTerminal, "ctrl-n");
        assert!(shortcuts.is_default(Command::NewTerminal));
        shortcuts.reset(Command::NewSession);
        assert!(shortcuts.is_empty());

        let saved: Shortcuts = serde_json::from_str(r#"{"zoom_in": ["Ctrl-="]}"#).unwrap();
        assert!(saved.validate().is_err());
        let saved: Shortcuts = serde_json::from_str(r#"{"launch": []}"#).unwrap();
        assert!(saved.validate().is_err());
        let saved: Shortcuts = serde_json::from_str(r#"{"new_terminal": ["ctrl-n"]}"#).unwrap();
        assert!(saved.validate().is_err());
    }
}
