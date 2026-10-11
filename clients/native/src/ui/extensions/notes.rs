use std::path::PathBuf;
use std::time::Duration;
use std::{env, fs, io};

use gpui_kit::component::input::{Editor, EditorState, InputEvent};
use gpui_kit::component::*;
use gpui_kit::*;

use super::PageView;
use super::host::Host;

pub(super) const ID: &str = "notes";
const SAVE_DELAY: Duration = Duration::from_millis(400);

enum NotesStatus {
    Idle,
    Saved,
    Failed(String),
}

/// Personal notes kept by this client, shared by all workspaces.
pub(super) struct NotesPage {
    editor: Entity<EditorState>,
    status: NotesStatus,
    save: Option<Task<()>>,
    _subscription: Subscription,
}

pub(super) fn build(_: Host, window: &mut Window, cx: &mut App) -> PageView {
    let page = cx.new(|cx| NotesPage::new(window, cx));
    let focus = page.read(cx).editor.read(cx).focus_handle(cx);
    PageView {
        view: page.into(),
        focus,
    }
}

pub(super) fn build_badge(host: Host, _: &mut Window, cx: &mut App) -> AnyView {
    super::session_badges::plain_badge(host, ID, cx)
}

impl NotesPage {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (text, status) = match load() {
            Ok(text) => (text, NotesStatus::Idle),
            Err(error) => (String::new(), NotesStatus::Failed(error.to_string())),
        };
        let editor = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .language("text")
                .line_number(false)
                .soft_wrap(true)
                .placeholder("Идеи, ссылки, что проверить…");
            editor.set_value(text, window, cx);
            editor
        });
        let subscription = cx.subscribe(&editor, |this, editor, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let text = editor.read(cx).value().to_string();
                this.schedule_save(text, cx);
            }
        });
        Self {
            editor,
            status,
            save: None,
            _subscription: subscription,
        }
    }

    fn schedule_save(&mut self, text: String, cx: &mut Context<Self>) {
        self.save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let result = cx
                .background_executor()
                .spawn(async move { store(&text) })
                .await;
            this.update(cx, |this, cx| {
                this.status = match result {
                    Ok(()) => NotesStatus::Saved,
                    Err(error) => NotesStatus::Failed(error.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }
}

impl Render for NotesPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (status, color) = match &self.status {
            NotesStatus::Idle => (
                "Сохраняются в этом клиенте".to_owned(),
                theme.muted_foreground,
            ),
            NotesStatus::Saved => ("Сохранено".to_owned(), theme.muted_foreground),
            NotesStatus::Failed(error) => (
                format!("Не удалось сохранить заметку: {error}"),
                theme.danger,
            ),
        };
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .px_6()
                    .pt_4()
                    .pb_2()
                    .gap_3()
                    .child(div().flex_1().font_semibold().child("Заметки"))
                    .child(div().text_xs().text_color(color).child(status)),
            )
            .child(
                div().flex_1().min_h_0().px_4().pb_4().child(
                    Editor::new(&self.editor)
                        .bordered(false)
                        .appearance(false)
                        .h_full(),
                ),
            )
    }
}

fn notes_path() -> io::Result<PathBuf> {
    let data = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .ok_or_else(|| io::Error::other("HOME is not set"))?;
    Ok(data.join("dev.proteus.native").join("notes.md"))
}

fn load() -> io::Result<String> {
    match fs::read_to_string(notes_path()?) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        result => result,
    }
}

fn store(text: &str) -> io::Result<()> {
    let path = notes_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)
}
