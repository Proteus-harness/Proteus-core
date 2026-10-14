//! Search and keyboard control of the session list: the filter narrows it
//! by title, arrows move the cursor, Enter opens the session under it and
//! F2 renames it.

use std::path::PathBuf;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;

use crate::ui::workspace::Workspace;

const LIST_CONTEXT: &str = "ProteusSessionList";

gpui_kit::actions!(
    proteus_sessions,
    [
        SelectNext,
        SelectPrevious,
        SelectFirst,
        SelectLast,
        OpenSession,
        RenameSession,
        FocusFilter,
    ]
);

pub(in crate::ui) fn bind_keys(cx: &mut App) {
    let context = Some(LIST_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, context),
        KeyBinding::new("up", SelectPrevious, context),
        KeyBinding::new("home", SelectFirst, context),
        KeyBinding::new("end", SelectLast, context),
        KeyBinding::new("enter", OpenSession, context),
        KeyBinding::new("f2", RenameSession, context),
        KeyBinding::new("ctrl-f", FocusFilter, context),
    ]);
}

pub(in crate::ui) struct SessionList {
    pub filter: Entity<InputState>,
    /// What the filter holds, trimmed.
    pub search: String,
    /// The row the arrows are on, when they moved.
    pub cursor: Option<PathBuf>,
    /// Rows as last shown, with the index of each among the list's
    /// children, so the cursor follows what is on screen.
    pub shown: Vec<(PathBuf, usize)>,
    pub scroll: ScrollHandle,
    pub focus: FocusHandle,
}

impl SessionList {
    pub(in crate::ui) fn new(
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> (Self, Subscription) {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Поиск по сессиям…"));
        let events = cx.subscribe_in(
            &filter,
            window,
            |this, state, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.session_list.search = state.read(cx).value().trim().to_string();
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.enter_session_list(window, cx),
                _ => {}
            },
        );
        let list = Self {
            filter,
            search: String::new(),
            cursor: None,
            shown: Vec::new(),
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
        };
        (list, events)
    }
}

impl Workspace {
    /// Registers the list keys on the panel.
    pub(super) fn session_list_actions(&self, element: Div, cx: &mut Context<Self>) -> Div {
        element
            .key_context(LIST_CONTEXT)
            .track_focus(&self.session_list.focus)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.step_session(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.step_session(-1, cx)))
            .on_action(
                cx.listener(|this, _: &SelectFirst, _, cx| this.step_session(isize::MIN, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectLast, _, cx| this.step_session(isize::MAX, cx)))
            .on_action(cx.listener(|this, _: &OpenSession, window, cx| {
                if let Some(dir) = this.session_cursor() {
                    this.select_session(dir, 2, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &RenameSession, window, cx| {
                if let Some(dir) = this.session_cursor() {
                    this.start_rename(dir, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
                this.session_list
                    .filter
                    .update(cx, |filter, cx| filter.focus(window, cx));
            }))
    }

    /// The row under the cursor: where the arrows left it, or the open
    /// session.
    pub(super) fn session_cursor(&self) -> Option<PathBuf> {
        let shown = &self.session_list.shown;
        let cursor = self.session_list.cursor.as_deref();
        let at = |dir| shown.iter().find(|(row, _)| row == dir);
        cursor
            .and_then(at)
            .or_else(|| self.active_chat_dir().and_then(at))
            .map(|(dir, _)| dir.clone())
    }

    /// Moves the cursor by `offset` rows; `isize::MIN`/`MAX` jump to the ends.
    fn step_session(&mut self, offset: isize, cx: &mut Context<Self>) {
        let shown = &self.session_list.shown;
        if shown.is_empty() {
            return;
        }
        let last = shown.len() as isize - 1;
        let current = self
            .session_cursor()
            .and_then(|dir| shown.iter().position(|(row, _)| *row == dir));
        let next = match current {
            Some(current) => (current as isize).saturating_add(offset).clamp(0, last),
            None if offset > 0 => 0,
            None => last,
        } as usize;
        let (dir, child) = shown[next].clone();
        self.session_list.scroll.scroll_to_item(child);
        self.session_list.cursor = Some(dir);
        cx.notify();
    }

    /// Enter in the filter hands the keyboard over to the list, on its
    /// first row.
    fn enter_session_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session_list.focus.focus(window, cx);
        self.session_list.cursor = None;
        self.step_session(isize::MIN, cx);
    }
}
