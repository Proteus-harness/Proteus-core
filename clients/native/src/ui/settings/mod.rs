//! Agent settings screen: edits the active profile through `/config/builder`.

mod fields;
mod nav;
mod page;
mod rows;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::app_protocol::config_builder::ConfigBuilderSnapshot;
use serde_json::Value;

use crate::api::Api;
use crate::settings::{
    Draft, FieldKind, FieldPath, Item, Page, TextFormat, default_hint, field_kind, format_value,
    parse_text,
};

const MESSAGE_TIMEOUT: Duration = Duration::from_secs(4);

pub enum SettingsEvent {
    Saved,
    Close,
}

enum LoadState {
    Loading,
    Failed(SharedString),
    Ready(Box<ConfigBuilderSnapshot>),
}

#[derive(Clone)]
enum TextInput {
    Line(Entity<InputState>),
    Area(Entity<TextareaState>),
}

struct TextField {
    input: TextInput,
    error: Option<SharedString>,
    _subscription: Subscription,
}

struct Message {
    text: SharedString,
    error: bool,
}

pub struct SettingsView {
    api: Api,
    session_dir: PathBuf,
    state: LoadState,
    baseline: Draft,
    draft: Draft,
    inputs: HashMap<FieldPath, TextField>,
    saving: bool,
    message: Option<Message>,
    message_timer: Option<Task<()>>,
    page: Option<String>,
    search: Entity<InputState>,
    query: String,
    open_advanced: HashSet<String>,
    focus_handle: FocusHandle,
    _search_subscription: Subscription,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(
        api: Api,
        session_dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Поиск настроек"));
        let search_subscription = cx.subscribe_in(&search, window, |this, state, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.query = state.read(cx).value().trim().to_string();
                cx.notify();
            }
        });
        // Keeps window shortcuts routed while no input inside the screen is focused.
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        let mut view = Self {
            api,
            session_dir,
            state: LoadState::Loading,
            baseline: Draft::default(),
            draft: Draft::default(),
            inputs: HashMap::new(),
            saving: false,
            message: None,
            message_timer: None,
            page: None,
            search,
            query: String::new(),
            open_advanced: HashSet::new(),
            focus_handle,
            _search_subscription: search_subscription,
        };
        view.load(cx);
        view
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.state = LoadState::Loading;
        let api = self.api.clone();
        let dir = self.session_dir.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { api.config_builder(&dir) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(snapshot) => this.accept(snapshot),
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
        self.inputs.clear();
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

    fn has_errors(&self) -> bool {
        self.inputs.values().any(|field| field.error.is_some())
    }

    fn can_save(&self) -> bool {
        self.snapshot().is_some_and(|snapshot| snapshot.writable)
            && !self.saving
            && self.dirty()
            && !self.has_errors()
    }

    fn update_draft(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Draft)) {
        change(&mut self.draft);
        self.clear_message();
        cx.notify();
    }

    fn set_field(&mut self, path: &FieldPath, value: Option<Value>, cx: &mut Context<Self>) {
        self.update_draft(cx, |draft| draft.set_value(path, value));
    }

    fn reset_field(&mut self, path: &FieldPath, cx: &mut Context<Self>) {
        self.inputs.remove(path);
        self.set_field(path, None, cx);
    }

    fn discard(&mut self, cx: &mut Context<Self>) {
        self.draft = self.baseline.clone();
        self.inputs.clear();
        self.clear_message();
        cx.notify();
    }

    fn select_page(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.page = Some(id.to_string());
        if !self.query.is_empty() {
            self.query.clear();
            self.search
                .update(cx, |search, cx| search.set_value("", window, cx));
        }
        cx.notify();
    }

    fn toggle_advanced(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.open_advanced.remove(key) {
            self.open_advanced.insert(key.to_string());
        }
        cx.notify();
    }

    fn clear_message(&mut self) {
        self.message = None;
        self.message_timer = None;
    }

    fn show_message(&mut self, text: impl Into<SharedString>, error: bool, cx: &mut Context<Self>) {
        self.message = Some(Message {
            text: text.into(),
            error,
        });
        self.message_timer = (!error).then(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(MESSAGE_TIMEOUT).await;
                this.update(cx, |this, cx| {
                    this.message = None;
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
        self.clear_message();
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
                        this.show_message(
                            "Сохранено · новые запросы используют обновлённую сборку",
                            false,
                            cx,
                        );
                        cx.emit(SettingsEvent::Saved);
                    }
                    Err(error) => {
                        this.show_message(format!("Не сохранено: {error:#}"), true, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Text controls keep their own state; create them once per field and
    /// drop them whenever the draft value changes from outside the control.
    fn ensure_inputs(&mut self, pages: &[Page], window: &mut Window, cx: &mut Context<Self>) {
        let fields = pages
            .iter()
            .flat_map(|page| &page.groups)
            .flat_map(|group| group.items.iter().chain(&group.advanced))
            .filter_map(|item| match item {
                Item::Field { path, field } => match field_kind(&field.value) {
                    FieldKind::Text(format) => Some((path, field, format)),
                    _ => None,
                },
                _ => None,
            });
        for (path, field, format) in fields {
            if self.inputs.contains_key(path) {
                continue;
            }
            let text = self
                .draft
                .value(path)
                .map(|value| format_value(format, value))
                .unwrap_or_default();
            let placeholder = default_hint(field)
                .map(|hint| format!("По умолчанию: {hint}"))
                .unwrap_or_else(|| "Не задано".to_string());
            let (input, subscription) = if format.is_multiline() {
                let state = cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .auto_grow(2, 12)
                        .placeholder(placeholder)
                        .default_value(text)
                });
                let path = path.clone();
                let subscription =
                    cx.subscribe_in(&state, window, move |this, state, event, _, cx| {
                        if matches!(event, InputEvent::Change) {
                            let text = state.read(cx).value();
                            this.text_changed(&path, format, &text, cx);
                        }
                    });
                (TextInput::Area(state), subscription)
            } else {
                let state = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .masked(format == TextFormat::Secret)
                        .default_value(text)
                });
                let path = path.clone();
                let subscription =
                    cx.subscribe_in(&state, window, move |this, state, event, _, cx| {
                        if matches!(event, InputEvent::Change) {
                            let text = state.read(cx).value();
                            this.text_changed(&path, format, &text, cx);
                        }
                    });
                (TextInput::Line(state), subscription)
            };
            self.inputs.insert(
                path.clone(),
                TextField {
                    input,
                    error: None,
                    _subscription: subscription,
                },
            );
        }
    }

    fn text_changed(
        &mut self,
        path: &FieldPath,
        format: TextFormat,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let parsed = parse_text(format, text);
        if let Some(field) = self.inputs.get_mut(path) {
            field.error = parsed.as_ref().err().map(|error| error.clone().into());
        }
        match parsed {
            Ok(value) => self.set_field(path, value, cx),
            Err(_) => cx.notify(),
        }
    }

    fn render_state(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        match &self.state {
            LoadState::Loading => centered(
                h_flex()
                    .gap_2()
                    .text_color(theme.muted_foreground)
                    .child(Spinner::new().small())
                    .child("Загрузка настроек…"),
            ),
            LoadState::Failed(error) => centered(
                v_flex()
                    .gap_3()
                    .items_center()
                    .max_w(px(520.))
                    .child(div().text_color(theme.danger).child(error.clone()))
                    .child(
                        Button::new("settings-retry")
                            .label("Повторить")
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                    ),
            ),
            LoadState::Ready(_) => div(),
        }
        .into_any_element()
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pages = match &self.state {
            LoadState::Ready(snapshot) => crate::settings::pages(snapshot, &self.draft),
            _ => Vec::new(),
        };
        self.ensure_inputs(&pages, window, cx);
        let content = if pages.is_empty() {
            self.render_state(cx)
        } else {
            self.render_content(&pages, cx).into_any_element()
        };

        h_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .child(self.render_nav(&pages, cx))
            .child(div().flex_1().min_w_0().h_full().child(content))
    }
}

fn centered(content: impl IntoElement) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(content)
}
