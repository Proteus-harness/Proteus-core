//! Settings screen: the agent profile (`/config/builder`) and client
//! preferences of this device.

mod addons;
mod catalog;
mod client;
mod dismiss;
mod editors;
mod extension;
mod extensions;
mod fields;
mod frame;
mod group;
mod hooks;
mod keys;
mod mcp;
mod mcp_editors;
mod mcp_form;
mod modules;
mod nav;
mod packages;
mod params;
mod reveal;
mod revisions;
mod search;
mod shortcuts;
mod sizes;
mod skills;
mod slots;
mod spot;
mod tools;
mod widgets;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::app_protocol::config_builder::{ConfigBuilderSnapshot, ConfigHistory};

use crate::api::Api;
use crate::settings::Draft;
use crate::settings::catalog::PageId;
use crate::settings::mcp::mcp_problems;
use editors::Editors;

pub(super) use keys::bind_keys;

const FEEDBACK_TIMEOUT: Duration = Duration::from_secs(4);

pub enum SettingsEvent {
    Saved,
    Close,
}

enum LoadState {
    Loading,
    Failed(SharedString),
    Ready(Box<ConfigBuilderSnapshot>),
}

enum HistoryState {
    Idle,
    Loading,
    Loaded(ConfigHistory),
    Failed(SharedString),
}

struct Feedback {
    text: SharedString,
    error: bool,
}

pub struct SettingsView {
    api: Api,
    session_dir: PathBuf,
    state: LoadState,
    /// A reload of a shown profile is running; the page stays in place.
    reloading: bool,
    addons: addons::AddonsState,
    baseline: Draft,
    draft: Draft,
    editors: Editors,
    /// Modules whose parameters are edited as raw JSON.
    json: HashSet<(String, String)>,
    /// Open disclosures by key.
    open: HashSet<String>,
    saving: bool,
    feedback: Option<Feedback>,
    feedback_timer: Option<Task<()>>,
    page: PageId,
    /// Scroll positions of the visited pages.
    scrolls: HashMap<String, ScrollHandle>,
    /// Settings search and the row its last hit points at.
    search: Entity<InputState>,
    query: String,
    spot: Option<spot::Spot>,
    history: HistoryState,
    tool_search: Entity<InputState>,
    tool_query: String,
    sizes: sizes::SizeSliders,
    catalogs: catalog::State,
    overview: crate::ui::analytics::Pages,
    /// Result of the last extension install or removal.
    extension_notice: Option<Feedback>,
    /// Installed extension whose removal waits for confirmation.
    removing: Option<String>,
    /// Command whose new shortcut is being recorded.
    recording: Option<(crate::shortcuts::Command, Subscription)>,
    focus_handle: FocusHandle,
    /// The page list, a single stop for Tab walked with the arrows.
    nav_focus: FocusHandle,
    /// The open page, holding focus between its controls.
    page_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(
        api: Api,
        session_dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tool_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Поиск по имени или описанию"));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Поиск настроек"));
        let (sizes, mut subscriptions) = sizes::SizeSliders::new(window, cx);
        let (catalogs, catalog_changes) = catalog::State::new(window, cx);
        subscriptions.push(catalog_changes);
        subscriptions.extend([
            cx.subscribe_in(
                &search,
                window,
                |this, state, event, window, cx| match event {
                    InputEvent::Change => {
                        this.query = state.read(cx).value().trim().to_string();
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => {
                        if let Some(hit) = this.search(&this.query, cx).into_iter().next() {
                            this.go_to(&hit, window, cx);
                        }
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(&tool_search, window, |this, state, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.tool_query = state.read(cx).value().trim().to_string();
                    cx.notify();
                }
            }),
        ]);
        // Keeps window shortcuts routed while no control inside the screen is focused.
        let focus_handle = cx.focus_handle();
        let nav_focus = cx.focus_handle().tab_stop(true);
        nav_focus.focus(window, cx);
        let overview =
            crate::ui::analytics::Pages::new(api.clone(), session_dir.clone(), window, cx);
        let mut view = Self {
            api,
            session_dir,
            state: LoadState::Loading,
            reloading: false,
            addons: addons::AddonsState::Loading {
                previous: None,
                _request: Task::ready(()),
            },
            baseline: Draft::default(),
            draft: Draft::default(),
            editors: Editors::default(),
            json: HashSet::new(),
            open: HashSet::new(),
            saving: false,
            feedback: None,
            feedback_timer: None,
            page: PageId::Model,
            scrolls: HashMap::new(),
            search,
            query: String::new(),
            spot: None,
            history: HistoryState::Idle,
            tool_search,
            tool_query: String::new(),
            sizes,
            catalogs,
            overview,
            extension_notice: None,
            removing: None,
            recording: None,
            focus_handle,
            nav_focus,
            page_focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        view.load(cx);
        view
    }

    /// Reads the profile; a shown one stays on screen until the new arrives.
    fn load(&mut self, cx: &mut Context<Self>) {
        if self.snapshot().is_some() {
            self.reloading = true;
        } else {
            self.state = LoadState::Loading;
        }
        self.load_addons(cx);
        self.refresh_history(cx);
        let api = self.api.clone();
        let dir = self.session_dir.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { api.config_builder(&dir) })
                .await;
            this.update(cx, |this, cx| {
                this.reloading = false;
                match result {
                    // Edits made while the profile was read win over it.
                    Ok(_) if this.dirty() => {}
                    Ok(snapshot) => this.accept(snapshot),
                    Err(error) if this.snapshot().is_some() => this.show_feedback(
                        format!("Не удалось перечитать профиль: {error:#}"),
                        true,
                        cx,
                    ),
                    Err(error) => this.state = LoadState::Failed(format!("{error:#}").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn accept(&mut self, snapshot: ConfigBuilderSnapshot) {
        self.baseline = Draft::from_snapshot(&snapshot);
        self.draft = self.baseline.clone();
        self.forget_editors();
        self.state = LoadState::Ready(Box::new(snapshot));
    }

    fn snapshot(&self) -> Option<&ConfigBuilderSnapshot> {
        match &self.state {
            LoadState::Ready(snapshot) => Some(snapshot),
            _ => None,
        }
    }

    fn dirty(&self) -> bool {
        self.draft.is_dirty(&self.baseline)
    }

    fn can_save(&self) -> bool {
        self.snapshot().is_some_and(|snapshot| snapshot.writable)
            && !self.saving
            && !self.catalogs.busy
            && self.dirty()
            && !self.has_errors()
    }

    /// Text that does not parse, or profile MCP servers that cannot be saved.
    fn has_errors(&self) -> bool {
        self.editors.has_errors() || !mcp_problems(&self.draft, self.addons()).is_empty()
    }

    fn update_draft(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Draft)) {
        change(&mut self.draft);
        self.clear_feedback();
        cx.notify();
    }

    /// Replaces the draft from outside the controls, so every editor is
    /// rebuilt from the new values.
    fn replace_draft(&mut self, draft: Draft, cx: &mut Context<Self>) {
        self.draft = draft;
        self.forget_editors();
        self.clear_feedback();
        cx.notify();
    }

    fn discard(&mut self, cx: &mut Context<Self>) {
        self.replace_draft(self.baseline.clone(), cx);
    }

    fn forget_editors(&mut self) {
        self.editors = Editors::default();
        self.json.clear();
    }

    fn select_page(&mut self, page: PageId, window: &mut Window, cx: &mut Context<Self>) {
        if page == PageId::Catalog {
            self.open_catalogs(cx);
            window.focus(&self.catalogs.focus, cx);
        }
        if page == PageId::History && matches!(self.history, HistoryState::Idle) {
            self.load_history(cx);
        }
        self.page = page;
        self.removing = None;
        self.spot = None;
        cx.notify();
    }

    fn toggle_open(&mut self, key: &str, cx: &mut Context<Self>) {
        let open = !self.open.remove(key);
        if open {
            self.open.insert(key.to_string());
        }
        crate::ui::motion::toggled(reveal_key(key), open, cx);
        cx.notify();
    }

    fn is_open(&self, key: &str) -> bool {
        self.open.contains(key)
    }

    /// Content under the disclosure `key`, animated after `toggle_open`.
    fn reveal(
        &self,
        key: &str,
        cx: &App,
        content: impl FnOnce() -> AnyElement,
    ) -> Option<AnyElement> {
        crate::ui::motion::reveal(reveal_key(key), self.is_open(key), cx, content)
    }

    fn clear_feedback(&mut self) {
        self.feedback = None;
        self.feedback_timer = None;
    }

    fn show_feedback(
        &mut self,
        text: impl Into<SharedString>,
        error: bool,
        cx: &mut Context<Self>,
    ) {
        self.feedback = Some(Feedback {
            text: text.into(),
            error,
        });
        self.feedback_timer = (!error).then(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(FEEDBACK_TIMEOUT).await;
                this.update(cx, |this, cx| {
                    this.feedback = None;
                    cx.notify();
                })
                .ok();
            })
        });
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if !self.can_save() {
            return;
        }
        let request = self.draft.changes(&self.baseline);
        let api = self.api.clone();
        let dir = self.session_dir.clone();
        self.saving = true;
        self.clear_feedback();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { api.save_config_builder(&dir, &request) })
                .await;
            this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(snapshot) => {
                        this.accept(snapshot);
                        this.load_addons(cx);
                        this.refresh_history(cx);
                        this.show_feedback(
                            "Сохранено · новые запросы используют обновлённую сборку",
                            false,
                            cx,
                        );
                        cx.emit(SettingsEvent::Saved);
                    }
                    Err(error) => {
                        this.show_feedback(format!("Не сохранено: {error:#}"), true, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads the history again on its page, or on the next visit.
    fn refresh_history(&mut self, cx: &mut Context<Self>) {
        if self.page == PageId::History {
            self.load_history(cx);
        } else {
            self.history = HistoryState::Idle;
        }
    }

    /// Loaded entries stay shown while a newer list is read.
    fn load_history(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.history, HistoryState::Loaded(_)) {
            self.history = HistoryState::Loading;
        }
        let api = self.api.clone();
        let dir = self.session_dir.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { api.config_history(&dir) })
                .await;
            this.update(cx, |this, cx| {
                this.history = match result {
                    Ok(history) => HistoryState::Loaded(history),
                    Err(error) => HistoryState::Failed(format!("{error:#}").into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.prepare_editors(window, cx);
        self.scrolls.entry(spot::page_key(&self.page)).or_default();
        crate::ui::motion::drive(window, cx);
        let screen = h_flex()
            .size_full()
            .key_context(super::SETTINGS_CONTEXT)
            .track_focus(&self.focus_handle);
        self.settings_keys(screen, cx)
            .on_action(
                cx.listener(|this, _: &super::NextSettingsPage, window, cx| {
                    this.step_page(1, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &super::PreviousSettingsPage, window, cx| {
                    this.step_page(-1, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &super::FocusSettingsSearch, window, cx| {
                    this.search.update(cx, |state, cx| state.focus(window, cx))
                }),
            )
            .on_action(
                cx.listener(|this, _: &super::DismissSettings, window, cx| {
                    this.dismiss(window, cx)
                }),
            )
            .child(self.render_nav(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.render_frame(cx)),
            )
    }
}

fn reveal_key(key: &str) -> SharedString {
    format!("settings-{key}").into()
}
