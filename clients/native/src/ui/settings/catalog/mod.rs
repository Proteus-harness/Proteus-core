//! Catalog screen state. Git and filesystem work stay off the UI thread.

mod actions;
mod view;
mod zip;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::text::TextViewState;
use gpui_kit::*;

use super::SettingsView;
use crate::catalog::{Installed, Source, schema::Catalog};

pub(super) struct State {
    pub url: Entity<InputState>,
    pub branch: Entity<InputState>,
    pub search: Entity<InputState>,
    pub query: String,
    pub category: Option<String>,
    pub loaded: Vec<(Source, Catalog)>,
    pub selected: Option<(Source, String)>,
    pub readme: Option<Entity<TextViewState>>,
    pub readme_changes: Option<Subscription>,
    pub busy: bool,
    pub opened: bool,
    pub message: Option<(bool, String)>,
    /// Receipts for profile edits become durable only with a successful save.
    pub pending: Vec<Installed>,
}

impl State {
    pub fn new(window: &mut Window, cx: &mut Context<SettingsView>) -> (Self, Subscription) {
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("URL git-репозитория"));
        let branch = cx.new(|cx| InputState::new(window, cx).placeholder("Ветка (необязательно)"));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Поиск в каталогах"));
        let changes = cx.subscribe(&search, |this, state, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.catalogs.query = state.read(cx).value().to_lowercase();
                cx.notify();
            }
        });
        (
            Self {
                url,
                branch,
                search,
                query: String::new(),
                category: None,
                loaded: Vec::new(),
                selected: None,
                readme: None,
                readme_changes: None,
                busy: false,
                opened: false,
                message: None,
                pending: Vec::new(),
            },
            changes,
        )
    }
}
