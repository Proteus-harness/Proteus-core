//! Agent settings screen: edits the active profile through `/config/builder`.

mod fields;
mod pages;

use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::setting::Settings;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::config_builder::ConfigBuilderSnapshot;
use serde_json::Value;

use crate::api::Api;
use crate::settings::{
    Draft, FieldKind, FieldPath, Item, Page, TextFormat, default_hint, field_kind, format_value,
    parse_text,
};

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

pub struct SettingsView {
    api: Api,
    session_dir: PathBuf,
    state: LoadState,
    baseline: Draft,
    draft: Draft,
    inputs: HashMap<FieldPath, TextField>,
    saving: bool,
    message: Option<(SharedString, bool)>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(api: Api, session_dir: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            api,
            session_dir,
            state: LoadState::Loading,
            baseline: Draft::default(),
            draft: Draft::default(),
            inputs: HashMap::new(),
            saving: false,
            message: None,
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
        self.message = None;
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
        self.message = None;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if !self.can_save() {
            return;
        }
        let request = self.draft.changes(&self.baseline);
        let api = self.api.clone();
        let dir = self.session_dir.clone();
        self.saving = true;
        self.message = None;
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
                        this.message = Some((
                            "Сохранено · новые запросы используют обновлённую сборку".into(),
                            false,
                        ));
                        cx.emit(SettingsEvent::Saved);
                    }
                    Err(error) => {
                        this.message = Some((format!("Не сохранено: {error:#}").into(), true));
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
            .flat_map(|group| &group.items)
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

    fn render_header(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let snapshot = self.snapshot();
        let target = snapshot.and_then(|snapshot| {
            snapshot
                .target_path
                .clone()
                .or(snapshot.config_path.clone())
        });
        let read_only = snapshot.is_some_and(|snapshot| !snapshot.writable);
        let dirty = self.dirty();
        let message = self.message.clone().map(|(text, error)| {
            div()
                .text_xs()
                .truncate()
                .text_color(if error { theme.danger } else { theme.success })
                .child(text)
        });

        h_flex()
            .h(px(44.))
            .flex_none()
            .px_4()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .child(div().text_sm().font_semibold().child("Настройки агента"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .truncate()
                    .text_color(theme.muted_foreground)
                    .when(read_only, |label| label.child("Профиль только для чтения"))
                    .when(!read_only, |label| label.children(target)),
            )
            .children(message)
            .child(
                Button::new("settings-discard")
                    .label("Отменить")
                    .ghost()
                    .small()
                    .disabled(!dirty || self.saving)
                    .on_click(cx.listener(|this, _, _, cx| this.discard(cx))),
            )
            .child(
                Button::new("settings-save")
                    .label("Сохранить")
                    .primary()
                    .small()
                    .loading(self.saving)
                    .disabled(!self.can_save())
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            )
            .child(
                Button::new("settings-close")
                    .icon(IconName::Close)
                    .ghost()
                    .small()
                    .tooltip("Закрыть настройки")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close))),
            )
    }

    fn render_warnings(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let snapshot = self.snapshot()?;
        if snapshot.warnings.is_empty() {
            return None;
        }
        let theme = cx.theme();
        Some(
            v_flex()
                .flex_none()
                .px_4()
                .py_2()
                .gap_1()
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.warning.opacity(0.08))
                .children(snapshot.warnings.iter().map(|warning| {
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .xsmall()
                                .text_color(theme.warning),
                        )
                        .child(warning.message.clone())
                })),
        )
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let body = match &self.state {
            LoadState::Loading => centered(
                h_flex()
                    .gap_2()
                    .text_color(theme.muted_foreground)
                    .child(Spinner::new().small())
                    .child("Загрузка настроек…"),
            )
            .into_any_element(),
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
            )
            .into_any_element(),
            LoadState::Ready(snapshot) => {
                let pages = crate::settings::pages(snapshot, &self.draft);
                self.ensure_inputs(&pages, window, cx);
                let pages = self.setting_pages(&pages, cx);
                Settings::new("agent-settings")
                    .sidebar_width(px(220.))
                    .pages(pages)
                    .into_any_element()
            }
        };

        v_flex()
            .size_full()
            .child(self.render_header(cx))
            .children(self.render_warnings(cx))
            .child(div().flex_1().min_h_0().child(body))
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
