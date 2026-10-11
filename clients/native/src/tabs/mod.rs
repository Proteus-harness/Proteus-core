//! Tabs of the main area: a tree of panes, each with its own tabs, split
//! side by side or stacked. Pure state, persisted with the preferences.
//!
//! Tabs are chats, addressed as `chat:<session dir>`, and documents of
//! extensions, addressed as `<extension>:<argument>`, e.g. `files:src/main.rs`.
//! No extension is called `chat`, so the two never mix.

mod geometry;
mod layout;
mod ops;
mod tree;

use std::path::Path;

pub use geometry::drop_edge;
pub use layout::TabLayout;
pub use tree::{Axis, Edge, Node, Pane, PaneId};

/// Prefix of chat tab ids.
const CHAT_PREFIX: &str = "chat:";

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

#[cfg(test)]
mod tests;
