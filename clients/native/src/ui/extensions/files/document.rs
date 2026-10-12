//! One workspace file in a tab: read-only text with highlighting, or its
//! diff against HEAD.

use crate::ui::kit;
use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::changes::{ChangesModel, changes_model};
use super::{path_label, status_color};
use crate::files::{DiffKind, FileContent, FileDiff, FileKind, language_for};
use crate::ui::extensions::{Host, HostEvent};
use crate::ui::kit::Hint as _;
use crate::ui::prefs;

const MAX_PREVIEW: &str = "512 КБ";

/// What a file tab shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui::extensions) enum Mode {
    File,
    Diff,
}

enum Preview {
    Loading,
    Text,
    Note(SharedString),
}

pub(in crate::ui::extensions) struct FileDocument {
    host: Host,
    path: String,
    session_dir: Option<PathBuf>,
    mode: Mode,
    preview: Preview,
    generation: u64,
    editor: Entity<EditorState>,
    changes: Entity<ChangesModel>,
    _subscriptions: Vec<Subscription>,
}

impl FileDocument {
    pub(in crate::ui::extensions) fn new(
        host: Host,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .language("text")
                .line_number(true)
                .soft_wrap(false);
            editor.set_readonly(true, cx);
            editor
        });
        let changes = changes_model(host.clone(), window, cx);
        let mut subscriptions = vec![cx.observe(&changes, |_, _, cx| cx.notify())];
        subscriptions.extend(host.subscribe_in(
            window,
            cx,
            |this, event, window, cx| match event {
                HostEvent::SessionOpened | HostEvent::RunSettled => this.load(window, cx),
            },
        ));
        cx.defer_in(window, |this, window, cx| this.load(window, cx));
        Self {
            host,
            path,
            session_dir: None,
            mode: Mode::File,
            preview: Preview::Loading,
            generation: 0,
            editor,
            changes,
            _subscriptions: subscriptions,
        }
    }

    pub(in crate::ui::extensions) fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.read(cx).focus_handle(cx)
    }

    pub(in crate::ui::extensions) fn set_mode(
        &mut self,
        mode: Mode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mode != mode {
            self.mode = mode;
            self.load(window, cx);
        }
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session_dir = self.host.session_dir(cx);
        let (Some(api), Some(dir)) = (self.host.api(cx), self.session_dir.clone()) else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        if !matches!(self.preview, Preview::Text) {
            self.preview = Preview::Loading;
        }
        cx.notify();
        let (mode, path) = (self.mode, self.path.clone());
        cx.spawn_in(window, async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    match mode {
                        Mode::File => api.workspace_file(&dir, &path).map(file_preview),
                        Mode::Diff => api.workspace_diff(&dir, &path).map(diff_preview),
                    }
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                this.preview = match loaded {
                    Ok(Loaded::Text { language, text }) => {
                        this.editor.update(cx, |editor, cx| {
                            editor.set_highlighter(language, cx);
                            if editor.value().as_ref() != text.as_str() {
                                editor.set_value(text, window, cx);
                            }
                        });
                        Preview::Text
                    }
                    Ok(Loaded::Note(note)) => Preview::Note(note.into()),
                    Err(error) => Preview::Note(format!("Не удалось открыть: {error:#}").into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn mode_button(&self, mode: Mode, label: &'static str, cx: &Context<Self>) -> Button {
        let id = match mode {
            Mode::File => "file-mode-file",
            Mode::Diff => "file-mode-diff",
        };
        kit::button(id)
            .label(label)
            .ghost()
            .xsmall()
            .selected(self.mode == mode)
            .on_click(cx.listener(move |this, _, window, cx| this.set_mode(mode, window, cx)))
    }
}

impl Render for FileDocument {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let status = self.changes.read(cx).status(&self.path);
        let header = h_flex()
            .h(prefs::scaled(36.))
            .flex_none()
            .px_4()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .child(
                path_label(&self.path, cx)
                    .flex_1()
                    .text_sm()
                    .font_family(theme.mono_font_family.clone()),
            )
            .when_some(status, |header, status| {
                header.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(status_color(status, cx))
                        .child(status.label()),
                )
            })
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(self.mode_button(Mode::File, "Файл", cx))
                    .child(self.mode_button(Mode::Diff, "Изменения", cx)),
            )
            .child(
                Clipboard::new("file-copy-path")
                    .value(SharedString::from(self.path.clone()))
                    .hint("Копировать путь", cx)
                    .xsmall(),
            );
        let body = match &self.preview {
            Preview::Loading => centered(Spinner::new(), cx).into_any_element(),
            Preview::Note(text) => centered(text.clone(), cx).into_any_element(),
            Preview::Text => div()
                .flex_1()
                .min_h_0()
                .child(
                    Editor::new(&self.editor)
                        .readonly(true)
                        .bordered(false)
                        .appearance(false)
                        .h_full(),
                )
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(header)
            .child(body)
    }
}

enum Loaded {
    Text {
        language: &'static str,
        text: String,
    },
    Note(String),
}

fn file_preview(file: FileContent) -> Loaded {
    match (file.kind, file.text) {
        (FileKind::Text, Some(text)) => Loaded::Text {
            language: language_for(&file.path).unwrap_or("text"),
            text,
        },
        (FileKind::TooLarge, _) => Loaded::Note(format!(
            "Файл занимает {}, просмотр доступен до {MAX_PREVIEW}.",
            size_label(file.size)
        )),
        _ => Loaded::Note(format!(
            "Двоичный файл ({}), просмотр недоступен.",
            size_label(file.size)
        )),
    }
}

fn diff_preview(diff: FileDiff) -> Loaded {
    match (diff.kind, diff.patch) {
        (DiffKind::Text, Some(patch)) if !patch.trim().is_empty() => Loaded::Text {
            language: "diff",
            text: patch,
        },
        (DiffKind::Text, _) => Loaded::Note("Отличий от HEAD нет.".into()),
        (DiffKind::TooLarge, _) => Loaded::Note("Файл слишком большой для сравнения.".into()),
        (DiffKind::Binary, _) => Loaded::Note("Двоичный файл, сравнение недоступно.".into()),
        (DiffKind::Unavailable, _) => Loaded::Note(format!(
            "Сравнение «{}» недоступно: нет git-репозитория или файла.",
            diff.path
        )),
    }
}

fn size_label(bytes: u64) -> String {
    const KIB: f64 = 1024.;
    let bytes = bytes as f64;
    let (value, unit) = if bytes < KIB {
        return format!("{bytes} Б");
    } else if bytes < KIB * KIB {
        (bytes / KIB, "КБ")
    } else {
        (bytes / KIB / KIB, "МБ")
    };
    format!("{value:.1} {unit}").replace('.', ",")
}

fn centered(content: impl IntoElement, cx: &App) -> Div {
    div()
        .flex_1()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .px_8()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(content)
}
