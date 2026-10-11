//! Tabs of the window: a tree of panes, each with its own tabs, split side
//! by side or stacked. Pure state, persisted with the preferences.
//!
//! Tabs are chats, addressed as `chat:<session dir>`; panels, addressed as
//! `panel:<name>`: the session list `panel:sessions` and the panel of an
//! extension `panel:<extension>`; and documents of extensions, addressed as
//! `<extension>:<argument>`, e.g. `files:src/main.rs`. No extension is
//! called `chat`, `panel` or `sessions`, so the kinds never mix.

mod geometry;
mod layout;
mod open;
mod ops;
mod pane;
mod pins;
mod size;
mod tree;

use std::path::Path;

pub use geometry::drop_edge;
pub use layout::TabLayout;
pub use pane::Pane;
pub use size::Size;
pub use tree::{Axis, Edge, Node, PaneId};

/// Prefix of chat tab ids.
const CHAT_PREFIX: &str = "chat:";
/// Prefix of panel tab ids.
const PANEL_PREFIX: &str = "panel:";

/// Panel of the session list.
pub const SESSIONS: &str = "sessions";

/// Panels of a fresh layout, by name; unavailable ones are dropped like any
/// other gone tab.
pub const DEFAULT_PANELS: [&str; 2] = [SESSIONS, "files"];

/// Width in pixels a pane of panels gets when it appears or is reset.
pub const PANEL_WIDTH: f32 = 264.;

/// Tab id of an extension document.
pub fn document(extension: &str, argument: &str) -> String {
    format!("{extension}:{argument}")
}

/// Extension and argument of a document tab; `None` for chats and panels.
pub fn split_document(id: &str) -> Option<(&str, &str)> {
    if is_chat(id) || is_panel(id) {
        return None;
    }
    id.split_once(':')
}

/// Tab id of the chat of an absolute session directory.
pub fn chat(session_dir: &Path) -> String {
    format!("{CHAT_PREFIX}{}", session_dir.to_string_lossy())
}

/// Session directory of a chat tab; `None` for other tabs.
pub fn chat_dir(id: &str) -> Option<&Path> {
    id.strip_prefix(CHAT_PREFIX).map(Path::new)
}

pub fn is_chat(id: &str) -> bool {
    id.starts_with(CHAT_PREFIX)
}

/// Tab id of a panel: [`SESSIONS`] or an extension id.
pub fn panel(name: &str) -> String {
    format!("{PANEL_PREFIX}{name}")
}

/// Name of a panel tab; `None` for other tabs.
pub fn panel_name(id: &str) -> Option<&str> {
    id.strip_prefix(PANEL_PREFIX)
}

pub fn is_panel(id: &str) -> bool {
    id.starts_with(PANEL_PREFIX)
}

#[cfg(test)]
mod tests;
