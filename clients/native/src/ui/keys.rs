//! The window commands' keymap, rebuilt from the shortcut preferences.

use gpui_kit::*;

use super::*;
use crate::shortcuts::{Command, Shortcuts};

fn binding(command: Command, chord: &str) -> KeyBinding {
    match command {
        Command::NewSession => KeyBinding::new(chord, NewSession, None),
        Command::NewTerminal => KeyBinding::new(chord, NewTerminal, None),
        Command::ToggleSettings => KeyBinding::new(chord, ToggleSettings, None),
        Command::ToggleSessions => KeyBinding::new(chord, ToggleSessions, None),
        Command::ToggleFiles => KeyBinding::new(chord, ToggleFiles, None),
        Command::NextTab => KeyBinding::new(chord, NextTab, None),
        Command::PreviousTab => KeyBinding::new(chord, PreviousTab, None),
        Command::SplitRight => KeyBinding::new(chord, SplitRight, None),
        Command::SplitDown => KeyBinding::new(chord, SplitDown, None),
        Command::FocusPaneLeft => KeyBinding::new(chord, FocusPaneLeft, None),
        Command::FocusPaneRight => KeyBinding::new(chord, FocusPaneRight, None),
        Command::FocusPaneUp => KeyBinding::new(chord, FocusPaneUp, None),
        Command::FocusPaneDown => KeyBinding::new(chord, FocusPaneDown, None),
        Command::ZoomIn => KeyBinding::new(chord, ZoomIn, None),
        Command::ZoomOut => KeyBinding::new(chord, ZoomOut, None),
        Command::ZoomReset => KeyBinding::new(chord, ZoomReset, None),
    }
}

fn command_of(action: &dyn Action) -> Option<Command> {
    Command::ALL
        .into_iter()
        .find(|command| binding(*command, "f1").action().name() == action.name())
}

/// Replaces the commands' bindings, keeping every other binding in order.
pub(super) fn bind(shortcuts: &Shortcuts, cx: &mut App) {
    let keymap = cx.key_bindings();
    let rest: Vec<KeyBinding> = keymap
        .borrow()
        .bindings()
        .filter(|binding| command_of(binding.action()).is_none())
        .cloned()
        .collect();
    let commands = Command::ALL.into_iter().flat_map(|command| {
        shortcuts
            .chords(command)
            .into_iter()
            .filter(|chord| Keystroke::parse(chord).is_ok())
            .map(move |chord| binding(command, &chord))
    });
    let mut keymap = keymap.borrow_mut();
    keymap.clear();
    keymap.add_bindings(rest.into_iter().chain(commands));
}

/// Whether the keystroke runs a command that wins over a focused terminal.
pub(in crate::ui) fn reserved_in_terminal(keystroke: &Keystroke, cx: &App) -> bool {
    cx.key_bindings()
        .borrow()
        .all_bindings_for_input(std::slice::from_ref(keystroke))
        .iter()
        .any(|binding| command_of(binding.action()).is_some_and(Command::in_terminal))
}
