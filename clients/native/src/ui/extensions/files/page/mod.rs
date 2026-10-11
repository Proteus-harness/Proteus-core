mod navigation;
mod preview;
mod sidebar;

use std::path::PathBuf;

use gpui_kit::component::h_flex;
use gpui_kit::component::input::{EditorState, InputEvent, InputState};
use gpui_kit::*;

use crate::files::{Changes, EntryKind, FileTree, Listing, TreeRow};
use crate::ui::extensions::{Host, HostEvent};

pub(in crate::ui::extensions) use navigation::bind_keys;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Tree,
    Changes,
}

/// What the preview shows for the selected file.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::ui::extensions) enum Mode {
    File,
    Diff,
}

enum Preview {
    Empty,
    Loading,
    Text,
    Note(SharedString),
}

/// Workspace browser: lazily listed tree with git marks, the list of
/// changes and a read-only preview of a file or its diff against HEAD.
pub(in crate::ui::extensions) struct FilesPage {
    host: Host,
    session_dir: Option<PathBuf>,
    tree: FileTree,
    rows: Vec<TreeRow>,
    changes: Option<Changes>,
    error: Option<SharedString>,
    tab: Tab,
    filter: Entity<InputState>,
    query: String,
    /// Row under the keyboard cursor; files under it are also `selected`.
    cursor: Option<String>,
    selected: Option<String>,
    mode: Mode,
    preview: Preview,
    generation: u64,
    editor: Entity<EditorState>,
    list: UniformListScrollHandle,
    changes_scroll: ScrollHandle,
    /// Focus of the list column; arrows and shortcuts work while it is held.
    pub(in crate::ui::extensions) focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl FilesPage {
    pub(in crate::ui::extensions) fn new(
        host: Host,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Фильтр по открытым папкам…"));
        let editor = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .language("text")
                .line_number(true)
                .soft_wrap(false);
            editor.set_readonly(true, cx);
            editor
        });
        let mut subscriptions = vec![cx.subscribe_in(
            &filter,
            window,
            |this, state, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.query = state.read(cx).value().trim().to_string();
                    this.sync_rows(cx);
                }
                InputEvent::PressEnter { .. } => this.enter_list(window, cx),
                _ => {}
            },
        )];
        subscriptions.extend(host.subscribe_in(
            window,
            cx,
            |this, event, window, cx| match event {
                HostEvent::SessionOpened => this.reset(window, cx),
                HostEvent::RunSettled => this.reload(window, cx),
                HostEvent::PanelOpened => {}
            },
        ));
        cx.defer_in(window, |this, window, cx| this.reset(window, cx));
        Self {
            host,
            session_dir: None,
            tree: FileTree::default(),
            rows: Vec::new(),
            changes: None,
            error: None,
            tab: Tab::Tree,
            filter,
            query: String::new(),
            cursor: None,
            selected: None,
            mode: Mode::File,
            preview: Preview::Empty,
            generation: 0,
            editor,
            list: UniformListScrollHandle::new(),
            changes_scroll: ScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// Shows `path` in the tree and opens it in the preview.
    pub(in crate::ui::extensions) fn reveal(
        &mut self,
        path: &str,
        mode: Mode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab = Tab::Tree;
        self.tree.reveal(path);
        self.sync_rows(cx);
        self.cursor = Some(path.to_string());
        self.select(path.to_string(), mode, window, cx);
        if let Some(index) = self.rows.iter().position(|row| row.path == path) {
            self.list.scroll_to_item(index, ScrollStrategy::Center);
        }
    }

    /// Binds the page to the current session. The first call keeps what
    /// [`FilesPage::reveal`] already asked for; a session switch starts over.
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session_dir.is_some() {
            self.tree = FileTree::default();
            self.changes = None;
            self.cursor = None;
            self.selected = None;
            self.preview = Preview::Empty;
            self.generation += 1;
        }
        self.session_dir = self.host.session_dir(cx);
        self.error = None;
        self.sync_rows(cx);
        self.load_changes(cx);
        if let Some(path) = self.selected.clone() {
            self.load_preview(path, window, cx);
        }
    }

    /// Re-reads the listed directories, git status and the open preview.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((api, dir)) = self.connection(cx) else {
            return;
        };
        let listed = self.tree.listed();
        cx.spawn(async move |this, cx| {
            let job_dir = dir.clone();
            let results = cx
                .background_executor()
                .spawn(async move {
                    listed
                        .into_iter()
                        .map(|path| {
                            let result = api.workspace_list(&job_dir, &path);
                            (path, result)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.session_dir.as_ref() != Some(&dir) {
                    return;
                }
                for (path, result) in results {
                    match result {
                        Ok(listing) => this.tree.insert(listing),
                        Err(_) if !path.is_empty() => this.tree.remove(&path),
                        Err(error) => this.error = Some(format!("{error:#}").into()),
                    }
                }
                this.sync_rows(cx);
            });
        })
        .detach();
        self.load_changes(cx);
        if let Some(path) = self.selected.clone() {
            self.load_preview(path, window, cx);
        }
    }

    fn connection(&self, cx: &App) -> Option<(crate::api::Api, PathBuf)> {
        Some((self.host.api(cx)?, self.session_dir.clone()?))
    }

    /// Rebuilds visible rows and requests listings they still need.
    fn sync_rows(&mut self, cx: &mut Context<Self>) {
        self.rows = self.tree.rows(&self.query);
        cx.notify();
        let Some((api, dir)) = self.connection(cx) else {
            return;
        };
        for path in self.tree.missing_listings(&self.rows) {
            self.tree.begin_loading(&path);
            let api = api.clone();
            let dir = dir.clone();
            cx.spawn(async move |this, cx| {
                let job_dir = dir.clone();
                let job_path = path.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { api.workspace_list(&job_dir, &job_path) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.session_dir.as_ref() != Some(&dir) {
                        return;
                    }
                    this.listing_loaded(&path, result);
                    this.sync_rows(cx);
                });
            })
            .detach();
        }
    }

    fn listing_loaded(&mut self, path: &str, result: anyhow::Result<Listing>) {
        match result {
            Ok(listing) => {
                if path.is_empty() {
                    self.error = None;
                }
                self.tree.insert(listing);
            }
            Err(error) => {
                self.tree.finish_loading(path);
                self.error = Some(format!("Не удалось прочитать «{path}»: {error:#}").into());
            }
        }
    }

    fn load_changes(&mut self, cx: &mut Context<Self>) {
        let Some((api, dir)) = self.connection(cx) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let job_dir = dir.clone();
            let result = cx
                .background_executor()
                .spawn(async move { api.workspace_changes(&job_dir) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.session_dir.as_ref() != Some(&dir) {
                    return;
                }
                match result {
                    Ok(changes) => {
                        this.tree.set_changes(&changes);
                        this.changes = Some(changes);
                    }
                    Err(error) => {
                        this.error =
                            Some(format!("Не удалось получить изменения: {error:#}").into())
                    }
                }
                this.sync_rows(cx);
            });
        })
        .detach();
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    fn select(&mut self, path: String, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.as_deref() == Some(path.as_str()) && self.mode == mode {
            return;
        }
        self.selected = Some(path.clone());
        self.mode = mode;
        self.load_preview(path, window, cx);
    }

    fn set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        if let Some(path) = self.selected.clone() {
            self.load_preview(path, window, cx);
        }
    }

    fn load_preview(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some((api, dir)) = self.connection(cx) else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        self.preview = Preview::Loading;
        cx.notify();
        let mode = self.mode;
        cx.spawn_in(window, async move |this, cx| {
            let job_path = path.clone();
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    match mode {
                        Mode::File => api
                            .workspace_file(&dir, &job_path)
                            .map(preview::file_preview),
                        Mode::Diff => api
                            .workspace_diff(&dir, &job_path)
                            .map(preview::diff_preview),
                    }
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                this.preview = match loaded {
                    Ok(preview::Loaded::Text { language, text }) => {
                        this.editor.update(cx, |editor, cx| {
                            editor.set_highlighter(language, cx);
                            editor.set_value(text, window, cx);
                        });
                        Preview::Text
                    }
                    Ok(preview::Loaded::Note(note)) => Preview::Note(note.into()),
                    Err(error) => Preview::Note(format!("Не удалось открыть: {error:#}").into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn click_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(index).cloned() else {
            return;
        };
        self.focus_handle.focus(window, cx);
        self.cursor = Some(row.path.clone());
        if row.kind == EntryKind::Directory {
            self.tree.toggle(&row.path);
            self.sync_rows(cx);
        } else {
            self.select(row.path, Mode::File, window, cx);
        }
    }

    fn click_change(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        self.cursor = Some(path.clone());
        self.select(path, Mode::Diff, window, cx);
    }
}

impl Render for FilesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .size_full()
            .child(self.render_sidebar(window, cx))
            .child(self.render_preview(cx))
    }
}
