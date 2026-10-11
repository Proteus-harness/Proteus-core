//! Keyboard control of the list: arrows move the cursor and preview files
//! under it, Left/Right fold directories, Enter keeps the file open.

use gpui_kit::*;

use super::{FilesPanel, Tab};
use crate::files::EntryKind;

pub(super) const LIST_CONTEXT: &str = "ProteusFilesList";

gpui_kit::actions!(
    proteus_files,
    [
        SelectNext,
        SelectPrevious,
        SelectFirst,
        SelectLast,
        ExpandEntry,
        CollapseEntry,
        OpenEntry,
        ToggleEntry,
        FocusFilter,
    ]
);

pub(in crate::ui::extensions) fn bind_keys(cx: &mut App) {
    let context = Some(LIST_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, context),
        KeyBinding::new("up", SelectPrevious, context),
        KeyBinding::new("home", SelectFirst, context),
        KeyBinding::new("end", SelectLast, context),
        KeyBinding::new("right", ExpandEntry, context),
        KeyBinding::new("left", CollapseEntry, context),
        KeyBinding::new("enter", OpenEntry, context),
        KeyBinding::new("space", ToggleEntry, context),
        KeyBinding::new("/", FocusFilter, context),
        KeyBinding::new("ctrl-f", FocusFilter, context),
    ]);
}

/// One navigable line of the current tab.
struct Stop {
    path: String,
    directory: bool,
    expanded: bool,
    depth: usize,
}

impl FilesPanel {
    /// Registers the list actions on the focused column.
    pub(super) fn list_actions(&self, element: Div, cx: &mut Context<Self>) -> Div {
        element
            .key_context(LIST_CONTEXT)
            .on_action(cx.listener(|this, _: &SelectNext, window, cx| this.step(1, window, cx)))
            .on_action(
                cx.listener(|this, _: &SelectPrevious, window, cx| this.step(-1, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectFirst, window, cx| this.step(isize::MIN, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectLast, window, cx| this.step(isize::MAX, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ExpandEntry, window, cx| this.expand(window, cx)))
            .on_action(cx.listener(|this, _: &CollapseEntry, window, cx| this.collapse(window, cx)))
            .on_action(
                cx.listener(|this, _: &OpenEntry, window, cx| this.activate(true, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ToggleEntry, window, cx| this.activate(false, window, cx)),
            )
            .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
                this.set_tab(Tab::Tree, cx);
                this.filter
                    .update(cx, |filter, cx| filter.focus(window, cx));
            }))
    }

    fn stops(&self, cx: &App) -> Vec<Stop> {
        match self.tab {
            Tab::Tree => self
                .rows
                .iter()
                .map(|row| Stop {
                    path: row.path.clone(),
                    directory: row.kind == EntryKind::Directory,
                    expanded: row.expanded,
                    depth: row.depth,
                })
                .collect(),
            Tab::Changes => self
                .changes
                .read(cx)
                .changes
                .iter()
                .flat_map(|changes| &changes.entries)
                .map(|change| Stop {
                    path: change.path.clone(),
                    directory: false,
                    expanded: false,
                    depth: 0,
                })
                .collect(),
        }
    }

    fn cursor_index(&self, stops: &[Stop]) -> Option<usize> {
        let cursor = self.cursor.as_deref().or(self.selected.as_deref())?;
        stops.iter().position(|stop| stop.path == cursor)
    }

    /// Moves the cursor by `offset` rows; `isize::MIN`/`MAX` jump to the ends.
    fn step(&mut self, offset: isize, window: &mut Window, cx: &mut Context<Self>) {
        let stops = self.stops(cx);
        if stops.is_empty() {
            return;
        }
        let last = stops.len() as isize - 1;
        let next = match self.cursor_index(&stops) {
            Some(current) => (current as isize).saturating_add(offset).clamp(0, last),
            None if offset > 0 => 0,
            None => last,
        };
        self.move_to(&stops, next as usize, window, cx);
    }

    fn move_to(
        &mut self,
        stops: &[Stop],
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let stop = &stops[index];
        self.cursor = Some(stop.path.clone());
        match self.tab {
            Tab::Tree => self.list.scroll_to_item(index, ScrollStrategy::Nearest),
            Tab::Changes => self.changes_scroll.scroll_to_item(index),
        }
        if !stop.directory {
            let mode = self.mode();
            self.open(&stop.path, mode, false, window, cx);
        }
        cx.notify();
    }

    /// Opens a folded directory or steps into an open one.
    fn expand(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stops = self.stops(cx);
        let Some(index) = self.cursor_index(&stops) else {
            return self.step(1, window, cx);
        };
        let stop = &stops[index];
        if !stop.directory {
            return;
        }
        if !stop.expanded {
            self.toggle_dir(&stop.path);
            self.sync_rows(cx);
        } else if stops
            .get(index + 1)
            .is_some_and(|next| next.depth > stop.depth)
        {
            self.move_to(&stops, index + 1, window, cx);
        }
    }

    /// Folds an open directory or moves to the parent one.
    fn collapse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stops = self.stops(cx);
        let Some(index) = self.cursor_index(&stops) else {
            return;
        };
        let stop = &stops[index];
        if stop.directory && stop.expanded {
            self.toggle_dir(&stop.path);
            self.sync_rows(cx);
            return;
        }
        let Some((parent, _)) = stop.path.rsplit_once('/') else {
            return;
        };
        if let Some(parent) = stops.iter().position(|stop| stop.path == parent) {
            self.move_to(&stops, parent, window, cx);
        }
    }

    /// Folds a directory, or opens the file: kept with focus for Enter,
    /// as a preview for Space.
    fn activate(&mut self, keep: bool, window: &mut Window, cx: &mut Context<Self>) {
        let stops = self.stops(cx);
        let Some(index) = self.cursor_index(&stops) else {
            return self.step(1, window, cx);
        };
        let stop = &stops[index];
        if stop.directory {
            self.toggle_dir(&stop.path);
            self.sync_rows(cx);
        } else {
            let (path, mode) = (stop.path.clone(), self.mode());
            self.open(&path, mode, keep, window, cx);
        }
    }

    /// Enter in the filter hands the keyboard over to the list.
    pub(super) fn enter_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        let stops = self.stops(cx);
        if self.cursor_index(&stops).is_none()
            && let Some(index) = stops.iter().position(|stop| !stop.directory)
        {
            self.move_to(&stops, index, window, cx);
        }
    }
}
