//! Keyboard control of the catalog: the arrows move through the list and
//! open the entry beside it, Enter installs or updates it, `/` and Ctrl+F
//! go to the search, and Enter or Down there come back to the list.

use gpui_kit::*;

use super::super::SettingsView;
use super::items::Status;
use crate::catalog::{Source, schema::Entry};
use crate::ui::prefs;

/// The whole page, the search included.
const PAGE_CONTEXT: &str = "ProteusCatalog";
/// The list with the details, where typed keys are not text.
const LIST_CONTEXT: &str = "ProteusCatalogList";
const SEARCH_CONTEXT: &str = "ProteusCatalogSearch";
const PAGE_STEP: isize = 10;

gpui_kit::actions!(
    proteus_catalog,
    [
        SelectNext,
        SelectPrevious,
        SelectFirst,
        SelectLast,
        PageDown,
        PageUp,
        Install,
        FocusSearch,
        FocusList,
    ]
);

pub(in crate::ui) fn bind_keys(cx: &mut App) {
    let (page, list) = (Some(PAGE_CONTEXT), Some(LIST_CONTEXT));
    let search = format!("{SEARCH_CONTEXT} > Input");
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, page),
        KeyBinding::new("up", SelectPrevious, page),
        KeyBinding::new("pagedown", PageDown, page),
        KeyBinding::new("pageup", PageUp, page),
        KeyBinding::new("ctrl-f", FocusSearch, page),
        KeyBinding::new("home", SelectFirst, list),
        KeyBinding::new("end", SelectLast, list),
        KeyBinding::new("enter", Install, list),
        KeyBinding::new("/", FocusSearch, list),
        // The single-line search swallows Down; it leads to the list instead.
        KeyBinding::new("down", FocusList, Some(search.as_str())),
    ]);
}

impl SettingsView {
    pub(super) fn catalog_page_keys(&self, page: Div, cx: &Context<Self>) -> Div {
        page.key_context(PAGE_CONTEXT)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.step_catalog(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.step_catalog(-1, cx)))
            .on_action(
                cx.listener(|this, _: &SelectFirst, _, cx| this.step_catalog(isize::MIN, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectLast, _, cx| this.step_catalog(isize::MAX, cx)))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| this.step_catalog(PAGE_STEP, cx)))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| this.step_catalog(-PAGE_STEP, cx)))
            .on_action(cx.listener(|this, _: &Install, _, cx| this.install_selected_catalog(cx)))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.catalogs
                    .search
                    .update(cx, |search, cx| search.focus(window, cx));
            }))
            .on_action(
                cx.listener(|this, _: &FocusList, window, cx| this.enter_catalog_list(window, cx)),
            )
    }

    pub(super) fn catalog_list_keys(&self, list: Div) -> Div {
        list.key_context(LIST_CONTEXT)
            .track_focus(&self.catalogs.focus)
    }

    pub(super) fn catalog_search_keys(search: Div) -> Div {
        search.key_context(SEARCH_CONTEXT)
    }

    /// Gives the keys to the list, with an entry under the cursor.
    pub(in crate::ui::settings) fn enter_catalog_list(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.catalogs.focus, cx);
        if self.catalog_cursor(&self.catalog_shown(cx)).is_none() {
            self.step_catalog(1, cx);
        }
    }

    /// Entries as the list shows them.
    fn catalog_shown(&self, cx: &App) -> Vec<(Source, Entry)> {
        self.catalog_items(&prefs::current(cx).catalogs.installed)
            .into_iter()
            .filter(|item| self.catalog_item_shown(item))
            .map(|item| (item.source.clone(), item.entry.clone()))
            .collect()
    }

    fn catalog_cursor(&self, shown: &[(Source, Entry)]) -> Option<usize> {
        let (source, id) = self.catalogs.selected.as_ref()?;
        shown
            .iter()
            .position(|(old, entry)| old == source && entry.id == *id)
    }

    /// Moves the selection by `offset` rows; `isize::MIN`/`MAX` jump to the ends.
    fn step_catalog(&mut self, offset: isize, cx: &mut Context<Self>) {
        let shown = self.catalog_shown(cx);
        let Some(last) = shown.len().checked_sub(1) else {
            return;
        };
        let next = match (self.catalog_cursor(&shown), offset) {
            (_, isize::MIN) => 0,
            (_, isize::MAX) => last,
            (Some(at), offset) => at.saturating_add_signed(offset).min(last),
            (None, offset) if offset > 0 => 0,
            (None, _) => last,
        };
        if self.catalog_cursor(&shown) == Some(next) {
            return;
        }
        self.catalogs.scroll.scroll_to_item(next);
        let (source, entry) = shown[next].clone();
        self.select_catalog_entry(source, entry, cx);
    }

    fn install_selected_catalog(&mut self, cx: &mut Context<Self>) {
        let Some((source, id)) = self.catalogs.selected.clone() else {
            return;
        };
        let installed = &prefs::current(cx).catalogs.installed;
        let Some(entry) = self
            .catalog_items(installed)
            .into_iter()
            .find(|item| {
                *item.source == source && item.entry.id == id && item.status != Status::Installed
            })
            .map(|item| item.entry.clone())
        else {
            return;
        };
        self.install_catalog_entry(source, entry, cx);
    }
}
