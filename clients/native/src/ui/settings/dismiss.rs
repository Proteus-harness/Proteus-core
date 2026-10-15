//! Esc on the settings screen undoes the nearest thing first: a pending
//! removal, a search, open forms; only then it leaves for the chat. Unsaved
//! changes keep the screen open, since leaving drops them.

use gpui_kit::component::input::InputState;
use gpui_kit::*;

use super::{SettingsEvent, SettingsView, reveal_key};
use crate::settings::catalog::PageId;

impl SettingsView {
    pub(super) fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.removing.take().is_some() {
            cx.notify();
            return;
        }
        if clear_focused(&self.tool_search.clone(), window, cx) {
            self.tool_query.clear();
            cx.notify();
            return;
        }
        if clear_focused(&self.catalogs.search.clone(), window, cx) {
            self.catalogs.query.clear();
            cx.notify();
            return;
        }
        if self.page == PageId::Catalog && self.catalogs.selected.is_some() {
            self.deselect_catalog_entry(cx);
            return;
        }
        if !self.search.read(cx).value().is_empty() {
            self.search
                .update(cx, |search, cx| search.set_value("", window, cx));
            self.query.clear();
            cx.notify();
            return;
        }
        if !self.open.is_empty() {
            for key in self.open.drain() {
                crate::ui::motion::toggled(reveal_key(&key), false, cx);
            }
            // The focused field may have been inside a form just closed.
            self.focus_handle.focus(window, cx);
            cx.notify();
            return;
        }
        if self.dirty() {
            let text = "Есть несохранённые изменения: сохраните или сбросьте их";
            // The catalog page has no save bar to show the warning.
            if self.page == PageId::Catalog {
                self.catalog_message(Err(anyhow::anyhow!(text)));
                cx.notify();
            } else {
                self.show_feedback(text, true, cx);
            }
            return;
        }
        cx.emit(SettingsEvent::Close);
    }
}

/// Empties a search field that holds the keys and some text.
fn clear_focused(input: &Entity<InputState>, window: &mut Window, cx: &mut App) -> bool {
    let state = input.read(cx);
    if state.value().is_empty() || !state.focus_handle(cx).is_focused(window) {
        return false;
    }
    input.update(cx, |input, cx| input.set_value("", window, cx));
    true
}
