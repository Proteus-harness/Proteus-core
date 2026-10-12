//! gpui-kit controls as the client builds them. A press of any mouse button
//! focuses a control, and the kit then rings it as if it was reached with
//! Tab; the client's controls keep focus without the ring. Hints are the
//! tooltips of controls, shown as the hints preference asks.

use gpui_kit::component::FocusableExt as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::switch::Switch;
use gpui_kit::{App, ElementId, SharedString};

use super::prefs;
use crate::preferences::Hints;
use crate::shortcuts::{self, Command};

pub(crate) fn button(id: impl Into<ElementId>) -> Button {
    Button::new(id).focus_ring(false)
}

pub(crate) fn switch(id: impl Into<ElementId>) -> Switch {
    Switch::new(id).focus_ring(false)
}

pub(crate) fn checkbox(id: impl Into<ElementId>) -> Checkbox {
    Checkbox::new(id).focus_ring(false)
}

/// Tooltip text of a control as the hints preference asks: what it does,
/// its shortcut, both or nothing.
pub(crate) fn hint_text(text: &str, keys: Option<&str>, cx: &App) -> Option<SharedString> {
    match (prefs::current(cx).hints, keys) {
        (Hints::Off, _) | (Hints::Keys, None) => None,
        (Hints::Keys, Some(keys)) => Some(keys.to_owned().into()),
        (Hints::Description, _) | (Hints::Full, None) => Some(text.to_owned().into()),
        (Hints::Full, Some(keys)) => Some(format!("{text} · {keys}").into()),
    }
}

/// The first shortcut of a window command, as people read it.
pub(crate) fn command_keys(command: Command, cx: &App) -> Option<String> {
    let chords = prefs::current(cx).shortcuts.chords(command);
    chords.first().map(|chord| shortcuts::label(chord))
}

pub(crate) trait Hint: Sized {
    /// A tooltip saying what the control does and how to reach it from
    /// the keyboard, filtered by the hints preference.
    fn hint_keys(self, text: &str, keys: Option<&str>, cx: &App) -> Self;

    fn hint(self, text: &str, cx: &App) -> Self {
        self.hint_keys(text, None, cx)
    }
}

impl Hint for Button {
    fn hint_keys(self, text: &str, keys: Option<&str>, cx: &App) -> Self {
        match hint_text(text, keys, cx) {
            Some(text) => self.tooltip(text),
            None => self,
        }
    }
}

impl Hint for Switch {
    fn hint_keys(self, text: &str, keys: Option<&str>, cx: &App) -> Self {
        match hint_text(text, keys, cx) {
            Some(text) => self.tooltip(text),
            None => self,
        }
    }
}

impl Hint for Clipboard {
    fn hint_keys(self, text: &str, keys: Option<&str>, cx: &App) -> Self {
        match hint_text(text, keys, cx) {
            Some(text) => self.tooltip(text),
            None => self,
        }
    }
}
