use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::input::Editor;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::sidebar::tab_button;
use super::{FilesPage, Mode, Preview};
use crate::files::{DiffKind, FileContent, FileDiff, FileKind, language_for};
use crate::ui::extensions::files::{path_label, status_color};

const MAX_PREVIEW: &str = "512 КБ";

impl FilesPage {
    pub(super) fn render_preview(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let Some(path) = self.selected.clone() else {
            return v_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(centered("Выберите файл слева.", cx));
        };
        let status = self.tree.status(&path);
        let header = h_flex()
            .h(px(44.))
            .flex_none()
            .px_4()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .child(
                path_label(&path, cx)
                    .flex_1()
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
                    .child(
                        tab_button("files-mode-file", "Файл", self.mode == Mode::File).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.set_mode(Mode::File, window, cx)
                            }),
                        ),
                    )
                    .child(
                        tab_button("files-mode-diff", "Изменения", self.mode == Mode::Diff)
                            .tooltip("Переключить: D")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_mode(Mode::Diff, window, cx)
                            })),
                    ),
            )
            .child(
                Clipboard::new("files-copy-path")
                    .value(SharedString::from(path.clone()))
                    .tooltip("Копировать путь")
                    .xsmall(),
            );
        let body = match &self.preview {
            Preview::Empty => centered("Выберите файл слева.", cx).into_any_element(),
            Preview::Loading => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new())
                .into_any_element(),
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
            .flex_1()
            .min_w_0()
            .h_full()
            .child(header)
            .child(body)
    }
}

pub(super) enum Loaded {
    Text {
        language: &'static str,
        text: String,
    },
    Note(String),
}

pub(super) fn file_preview(file: FileContent) -> Loaded {
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

pub(super) fn diff_preview(diff: FileDiff) -> Loaded {
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

fn centered(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .flex_1()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .px_8()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}
