//! Built-in terminal: a document tab per shell, `terminal:<n>`.

mod view;

use gpui_kit::*;

use super::{Host, SurfaceView};
use crate::tabs::TabLayout;

pub(super) const ID: &str = "terminal";

pub(super) fn build_document(
    host: Host,
    _argument: &str,
    window: &mut Window,
    cx: &mut App,
) -> SurfaceView {
    let view = cx.new(|cx| view::TerminalView::new(host, window, cx));
    SurfaceView {
        focus: view.read(cx).focus_handle(cx),
        view: view.into(),
    }
}

pub(super) fn document_label(argument: &str) -> SharedString {
    format!("Терминал {argument}").into()
}

/// Tab id of a new terminal: one past the highest open number.
pub(in crate::ui) fn next_tab(layout: &TabLayout) -> String {
    let prefix = format!("{ID}:");
    let last = layout
        .panes()
        .iter()
        .flat_map(|pane| &pane.tabs)
        .filter_map(|tab| tab.strip_prefix(&prefix)?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("{prefix}{}", last + 1)
}
