mod navigation;
mod render;
mod search;

use std::path::PathBuf;
use std::time::Instant;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;

use super::ID;
use super::changes::{ChangesModel, changes_model};
use super::document::{FileDocument, Mode};
use crate::files::{EntryKind, FileTree, Listing, SearchResult, TreeRow, search_rows};
use crate::ui::extensions::{Host, HostEvent, OpenDocument};

pub(in crate::ui::extensions) use navigation::bind_keys;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Tree,
    Changes,
}

/// Panel of the workspace files: lazily listed tree with git marks
/// and the list of changes. Files open as tabs: a single click shows a
/// preview tab that the next click replaces, a double click or Enter keeps it.
pub(in crate::ui::extensions) struct FilesPanel {
    host: Host,
    session_dir: Option<PathBuf>,
    tree: FileTree,
    rows: Vec<TreeRow>,
    changes: Entity<ChangesModel>,
    error: Option<SharedString>,
    tab: Tab,
    filter: Entity<InputState>,
    query: String,
    /// Whole-workspace matches of `query`; until they arrive the listed
    /// directories are filtered locally.
    search: Option<SearchResult>,
    /// Bumped by every query change so late answers are dropped.
    search_generation: u64,
    search_error: Option<SharedString>,
    /// The directory (`""` for the root) whose rows just appeared, so they
    /// enter one after another.
    entered: Option<(String, Instant)>,
    /// Row under the keyboard cursor.
    cursor: Option<String>,
    /// File last opened from this view.
    selected: Option<String>,
    /// Active file tab already revealed in the tree.
    followed: Option<String>,
    list: UniformListScrollHandle,
    changes_scroll: ScrollHandle,
    pub(in crate::ui::extensions) focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl FilesPanel {
    pub(in crate::ui::extensions) fn new(
        host: Host,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Поиск по проекту…"));
        let changes = changes_model(host.clone(), window, cx);
        let mut subscriptions = vec![
            cx.subscribe_in(
                &filter,
                window,
                |this, state, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        this.query = state.read(cx).value().trim().to_string();
                        this.schedule_search(cx);
                        this.sync_rows(cx);
                    }
                    InputEvent::PressEnter { .. } => this.enter_list(window, cx),
                    _ => {}
                },
            ),
            cx.observe(&changes, |this, changes, cx| {
                if let Some(snapshot) = &changes.read(cx).changes {
                    this.tree.set_changes(snapshot);
                }
                this.sync_rows(cx);
            }),
        ];
        subscriptions.extend(
            host.subscribe(cx, |this: &mut Self, event, cx| match event {
                HostEvent::SessionOpened => this.reset(cx),
                HostEvent::RunSettled => this.reload(cx),
            }),
        );
        cx.defer_in(window, |this, _, cx| this.reset(cx));
        Self {
            host,
            session_dir: None,
            tree: FileTree::default(),
            rows: Vec::new(),
            changes,
            error: None,
            tab: Tab::Tree,
            filter,
            query: String::new(),
            search: None,
            search_generation: 0,
            search_error: None,
            entered: Some((String::new(), Instant::now())),
            cursor: None,
            selected: None,
            followed: None,
            list: UniformListScrollHandle::new(),
            changes_scroll: ScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// Binds the view to the current session; a session switch starts over.
    fn reset(&mut self, cx: &mut Context<Self>) {
        let session_dir = self.host.session_dir(cx);
        if self.session_dir.is_some() && self.session_dir != session_dir {
            self.tree = FileTree::default();
            self.entered = Some((String::new(), Instant::now()));
            self.cursor = None;
            self.selected = None;
            self.followed = None;
            self.search = None;
        }
        self.session_dir = session_dir;
        self.error = None;
        self.schedule_search(cx);
        if let Some(snapshot) = &self.changes.read(cx).changes {
            self.tree.set_changes(snapshot);
        }
        self.sync_rows(cx);
    }

    /// Re-reads the listed directories and git status.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.changes.update(cx, |changes, cx| changes.refresh(cx));
        let Some((api, dir)) = self.connection(cx) else {
            return;
        };
        self.schedule_search(cx);
        self.tree.retry_failed();
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
    }

    fn connection(&self, cx: &App) -> Option<(crate::api::Api, PathBuf)> {
        Some((self.host.api(cx)?, self.session_dir.clone()?))
    }

    /// Selects the file whose tab is active and opens its folders, so the
    /// tree follows the tabs.
    fn follow_active(&mut self, cx: &mut Context<Self>) {
        let app: &App = cx;
        let active = self
            .host
            .read(app, |host| host.active_document(ID, app))
            .flatten();
        if active == self.followed {
            return;
        }
        self.followed = active.clone();
        let Some(path) = active else {
            return;
        };
        self.cursor = Some(path.clone());
        if self.selected.as_deref() == Some(path.as_str()) {
            return;
        }
        self.selected = Some(path.clone());
        self.tree.reveal(&path);
        self.sync_rows(cx);
        if let Some(index) = self.rows.iter().position(|row| row.path == path) {
            self.list.scroll_to_item(index, ScrollStrategy::Nearest);
        }
    }

    /// Rebuilds visible rows and requests listings they still need.
    fn sync_rows(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if let Some(result) = self
            .search
            .as_ref()
            .filter(|result| result.query == self.query)
        {
            self.rows = search_rows(&self.tree, result);
            return;
        }
        self.rows = self.tree.rows(&self.query);
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
                if let Some((entered, at)) = &mut self.entered
                    && entered == path
                {
                    *at = Instant::now();
                }
                self.tree.insert(listing);
            }
            Err(error) => {
                self.tree.fail(path);
                self.error = Some(format!("Не удалось прочитать «{path}»: {error:#}").into());
            }
        }
    }

    /// Expands or collapses `dir`; the rows of an expanded one enter.
    fn toggle_dir(&mut self, dir: &str) {
        if self.tree.toggle(dir) {
            self.entered = Some((dir.to_string(), Instant::now()));
        }
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    /// Opens `path` as a tab. A preview tab keeps the keyboard in the list;
    /// a kept one takes the focus.
    fn open(
        &mut self,
        path: &str,
        mode: Mode,
        keep: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = Some(path.to_string());
        cx.notify();
        let open = OpenDocument {
            preview: !keep,
            focus: keep,
        };
        let Some(view) = self.host.open_document(ID, path, open, window, cx) else {
            return;
        };
        if let Ok(document) = view.downcast::<FileDocument>() {
            document.update(cx, |document, cx| document.set_mode(mode, window, cx));
        }
    }

    fn mode(&self) -> Mode {
        match self.tab {
            Tab::Tree => Mode::File,
            Tab::Changes => Mode::Diff,
        }
    }

    fn click_row(&mut self, index: usize, keep: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(index).cloned() else {
            return;
        };
        self.cursor = Some(row.path.clone());
        if row.kind == EntryKind::Directory {
            self.focus_handle.focus(window, cx);
            if self.searching() {
                self.show_in_tree(&row.path, window, cx);
            } else if !keep {
                self.toggle_dir(&row.path);
                self.sync_rows(cx);
            }
        } else {
            if !keep {
                self.focus_handle.focus(window, cx);
            }
            self.open(&row.path, Mode::File, keep, window, cx);
        }
    }

    fn click_change(
        &mut self,
        path: String,
        keep: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !keep {
            self.focus_handle.focus(window, cx);
        }
        self.cursor = Some(path.clone());
        self.open(&path, Mode::Diff, keep, window, cx);
    }
}
