//! Catalog screen state. Git and filesystem work stay off the UI thread.

mod actions;
mod apply;
mod details;
mod items;
mod keys;
mod sources;
mod view;
mod zip;

pub(in crate::ui) use keys::bind_keys;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::text::TextViewState;
use gpui_kit::*;

use super::SettingsView;
use crate::catalog::{
    Source,
    schema::{Catalog, Kind},
};

pub(super) struct State {
    pub url: Entity<InputState>,
    pub branch: Entity<InputState>,
    pub search: Entity<InputState>,
    pub query: String,
    pub show: Show,
    pub kind: Option<Kind>,
    pub loaded: Vec<(Source, Catalog)>,
    pub selected: Option<(Source, String)>,
    pub readme: Option<Entity<TextViewState>>,
    pub readme_changes: Option<Subscription>,
    pub busy: bool,
    /// The entry whose install or removal holds `busy`.
    pub working: Option<(Source, String)>,
    /// Sources being downloaded; each shows up as soon as it is ready.
    pub loading: Vec<Source>,
    /// Errors of the downloads still running together.
    pub failures: Vec<String>,
    pub opened: bool,
    pub message: Option<(bool, String)>,
    /// The entry list; the arrows work while focus is on the page.
    pub focus: FocusHandle,
    pub scroll: ScrollHandle,
}

/// Which entries the list shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Show {
    All,
    Installed,
    Updates,
}

impl State {
    pub fn new(window: &mut Window, cx: &mut Context<SettingsView>) -> (Self, Subscription) {
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("URL git-репозитория"));
        let branch = cx.new(|cx| InputState::new(window, cx).placeholder("Ветка (необязательно)"));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Поиск в каталоге"));
        let changes = cx.subscribe_in(
            &search,
            window,
            |this, state, event, window, cx| match event {
                InputEvent::Change => {
                    this.catalogs.query = state.read(cx).value().to_lowercase();
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.enter_catalog_list(window, cx),
                _ => {}
            },
        );
        (
            Self {
                url,
                branch,
                search,
                query: String::new(),
                show: Show::All,
                kind: None,
                loaded: Vec::new(),
                selected: None,
                readme: None,
                readme_changes: None,
                busy: false,
                working: None,
                loading: Vec::new(),
                failures: Vec::new(),
                opened: false,
                message: None,
                focus: cx.focus_handle(),
                scroll: ScrollHandle::new(),
            },
            changes,
        )
    }
}

/// Agent Plugins and MCP servers live in the profile; the rest in client
/// folders.
pub(super) fn in_profile(kind: Kind) -> bool {
    matches!(kind, Kind::AgentPlugin | Kind::McpServer)
}
