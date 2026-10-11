use std::path::PathBuf;
use std::time::Duration;
use std::{env, fs, io};

use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::*;
use gpui_kit::*;

use super::frame::{is_open, widget_frame};
use super::host::Host;

pub(super) const ID: &str = "notes";
const SAVE_DELAY: Duration = Duration::from_millis(400);

enum NotesStatus {
    Idle,
    Saved,
    Failed(String),
}

/// Personal notes kept by this client, shared by all workspaces.
pub(super) struct NotesWidget {
    editor: Entity<TextareaState>,
    status: NotesStatus,
    save: Option<Task<()>>,
    _subscription: Subscription,
}

pub(super) fn build(_: Host, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| NotesWidget::new(window, cx)).into()
}

impl NotesWidget {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (text, status) = match load() {
            Ok(text) => (text, NotesStatus::Idle),
            Err(error) => (String::new(), NotesStatus::Failed(error.to_string())),
        };
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(6, 20)
                .placeholder("Идеи, ссылки, что проверить…")
                .default_value(text)
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

impl Render for NotesWidget {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = is_open(ID, cx).then(|| {
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
                .gap_1()
                .child(Textarea::new(&self.editor).small().w_full())
                .child(div().text_xs().text_color(color).child(status))
                .into_any_element()
        });
        widget_frame(ID, "Заметки".into(), None, body, cx)
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
