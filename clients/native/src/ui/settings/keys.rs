//! Keyboard control of the settings screen: the arrows walk the page list
//! and Enter leads into the page, Down in the search reaches the list or the
//! hits, the arrows walk the hits, and Ctrl+S saves the profile.

use gpui_kit::*;

use super::SettingsView;
use crate::settings::catalog::PageId;

/// The page list, which holds focus as one control.
const NAV_CONTEXT: &str = "ProteusSettingsNav";
const SEARCH_CONTEXT: &str = "ProteusSettingsSearch";
/// Search hits, each of them a control of its own.
const HITS_CONTEXT: &str = "ProteusSettingsHits";

gpui_kit::actions!(
    proteus_settings,
    [
        NextPage,
        PreviousPage,
        FirstPage,
        LastPage,
        OpenPage,
        LeaveSearch,
        NextHit,
        PreviousHit,
        Save,
    ]
);

pub(in crate::ui) fn bind_keys(cx: &mut App) {
    let (nav, hits) = (Some(NAV_CONTEXT), Some(HITS_CONTEXT));
    let search = format!("{SEARCH_CONTEXT} > Input");
    cx.bind_keys([
        KeyBinding::new("down", NextPage, nav),
        KeyBinding::new("up", PreviousPage, nav),
        KeyBinding::new("home", FirstPage, nav),
        KeyBinding::new("end", LastPage, nav),
        KeyBinding::new("enter", OpenPage, nav),
        KeyBinding::new("right", OpenPage, nav),
        // The single-line search swallows Down; it leads to the list instead.
        KeyBinding::new("down", LeaveSearch, Some(search.as_str())),
        KeyBinding::new("down", NextHit, hits),
        KeyBinding::new("up", PreviousHit, hits),
        KeyBinding::new("ctrl-s", Save, Some(crate::ui::SETTINGS_CONTEXT)),
    ]);
    super::catalog::bind_keys(cx);
}

impl SettingsView {
    pub(super) fn settings_keys(&self, screen: Div, cx: &Context<Self>) -> Div {
        screen
            .on_action(cx.listener(|this, _: &NextPage, window, cx| this.walk_pages(1, window, cx)))
            .on_action(
                cx.listener(|this, _: &PreviousPage, window, cx| this.walk_pages(-1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &FirstPage, window, cx| {
                this.walk_pages(isize::MIN, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &LastPage, window, cx| {
                    this.walk_pages(isize::MAX, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &OpenPage, window, cx| this.enter_page(window, cx)))
            .on_action(cx.listener(|_, _: &LeaveSearch, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &NextHit, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &PreviousHit, window, cx| window.focus_prev(cx)))
            .on_action(cx.listener(|this, _: &Save, _, cx| this.save(cx)))
    }

    pub(super) fn nav_keys(&self, list: Stateful<Div>) -> Stateful<Div> {
        list.key_context(NAV_CONTEXT).track_focus(&self.nav_focus)
    }

    pub(super) fn search_keys(search: Div) -> Div {
        search.key_context(SEARCH_CONTEXT)
    }

    pub(super) fn hits_keys(hits: Stateful<Div>) -> Stateful<Div> {
        hits.key_context(HITS_CONTEXT)
    }

    /// Moves through the page list without wrapping, focus staying on it.
    fn walk_pages(&mut self, offset: isize, window: &mut Window, cx: &mut Context<Self>) {
        let pages = self.page_list(cx);
        let current = pages.iter().position(|page| *page == self.page);
        let next = match (offset, current) {
            (isize::MIN, _) | (_, None) => 0,
            (isize::MAX, _) => pages.len().saturating_sub(1),
            (offset, Some(current)) => current
                .saturating_add_signed(offset)
                .min(pages.len().saturating_sub(1)),
        };
        if let Some(page) = pages.get(next).cloned()
            && page != self.page
        {
            self.select_page(page, window, cx);
            window.focus(&self.nav_focus, cx);
        }
    }

    /// Leaves the page list for the first control of the page.
    fn enter_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page == PageId::Catalog {
            self.enter_catalog_list(window, cx);
        } else {
            window.focus_next(cx);
        }
    }
}
