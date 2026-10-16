//! gpui-kit controls as the client builds them. A focused control is ringed
//! only while the keyboard drives the window, so one focused by a click or
//! by the client itself stays plain. Hints are the tooltips of controls,
//! shown as the hints preference asks.

use std::cell::Cell;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::FocusableExt as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::switch::Switch;
use gpui_kit::{App, ElementId, MouseButton, SharedString, StatefulInteractiveElement, Styled};

use super::prefs;
use crate::preferences::Hints;
use crate::shortcuts::{self, Command};

thread_local! {
    static KEYBOARD: Cell<bool> = const { Cell::new(false) };
}

/// Remembers whether the keyboard drives the window, for the controls built
/// in this frame. The window's root view calls it first in its render; a
/// switch between keyboard and mouse redraws every view, so no control is
/// built with a stale answer.
pub(crate) fn track_input(window: &gpui_kit::Window) {
    KEYBOARD.set(window.last_input_was_keyboard());
}

pub(crate) fn button(id: impl Into<ElementId>) -> Button {
    Button::new(id).focus_ring(KEYBOARD.get())
}

pub(crate) fn switch(id: impl Into<ElementId>) -> Switch {
    Switch::new(id).focus_ring(KEYBOARD.get())
}

pub(crate) fn checkbox(id: impl Into<ElementId>) -> Checkbox {
    Checkbox::new(id).focus_ring(KEYBOARD.get())
}

/// A clickable row or card that Tab reaches like a button: Enter or Space
/// presses it, and it is marked while the keyboard holds focus on it. A
/// mouse press leaves focus where it was, as with buttons.
pub(crate) trait Pressable: StatefulInteractiveElement + Styled {
    fn pressable(self, cx: &App) -> Self {
        let theme = cx.theme();
        let (ring, mark) = (theme.ring, theme.list_hover);
        self.focusable()
            .tab_stop(true)
            .focus_visible(move |style| style.bg(mark).border_color(ring))
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
    }
}

impl<E: StatefulInteractiveElement + Styled> Pressable for E {}

/// Moves focus to the next or the previous stop inside `container`, coming
/// round from its end to its start.
pub(crate) fn cycle_focus(
    container: &gpui_kit::FocusHandle,
    forward: bool,
    window: &mut gpui_kit::Window,
    cx: &mut App,
) {
    let start = window.focused(cx);
    // The container itself is no stop, so a walk from it never comes back
    // to it when it holds no stops either.
    for _ in 0..256 {
        if forward {
            window.focus_next(cx);
        } else {
            window.focus_prev(cx);
        }
        let focused = window.focused(cx);
        if container.contains_focused(window, cx) || focused == start {
            return;
        }
    }
}

/// Moves focus to the next or the previous stop inside `container`; at its
/// end the focus stays where it was, and the result says whether it moved.
pub(crate) fn step_focus(
    container: &gpui_kit::FocusHandle,
    forward: bool,
    window: &mut gpui_kit::Window,
    cx: &mut App,
) -> bool {
    let start = window.focused(cx);
    if forward {
        window.focus_next(cx);
    } else {
        window.focus_prev(cx);
    }
    if container.contains_focused(window, cx) && window.focused(cx) != start {
        return true;
    }
    if let Some(start) = start {
        window.focus(&start, cx);
    }
    false
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
