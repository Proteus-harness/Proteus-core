use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::*;
use gpui_kit::*;

use super::ID;
use crate::files::{ChangeStatus, Changes};
use crate::ui::extensions::badge::{Details, badge_frame, icon_with_count};
use crate::ui::extensions::shared::shared;
use crate::ui::extensions::{Host, HostEvent};

/// Git changes of the workspace, shared by the tree, file tabs and the icon.
pub(super) struct ChangesModel {
    host: Host,
    pub(super) changes: Option<Changes>,
    pub(super) error: Option<SharedString>,
    loading: bool,
    _events: Option<Subscription>,
}

pub(super) fn changes_model(host: Host, window: &mut Window, cx: &mut App) -> Entity<ChangesModel> {
    shared(cx, |cx| {
        cx.new(|cx| {
            let events = host.subscribe(cx, |this: &mut ChangesModel, event, cx| match event {
                HostEvent::SessionOpened => {
                    this.changes = None;
                    this.error = None;
                    this.refresh(cx)
                }
                HostEvent::RunSettled => this.refresh(cx),
            });
            cx.defer_in(window, |this, _, cx| this.refresh(cx));
            ChangesModel {
                host,
                changes: None,
                error: None,
                loading: false,
                _events: events,
            }
        })
    })
}

impl ChangesModel {
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let (Some(api), Some(dir)) = (self.host.api(cx), self.host.session_dir(cx)) else {
            return;
        };
        self.loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { api.workspace_changes(&dir) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(changes) => {
                        this.changes = Some(changes);
                        this.error = None;
                    }
                    Err(error) => this.error = Some(format!("{error:#}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn is_loading(&self) -> bool {
        self.loading
    }

    /// Number of changed files, when the workspace is a repository.
    pub(super) fn count(&self) -> Option<usize> {
        self.changes
            .as_ref()
            .filter(|changes| changes.repository)
            .map(|changes| changes.entries.len())
    }

    pub(super) fn status(&self, path: &str) -> Option<ChangeStatus> {
        self.changes
            .as_ref()?
            .entries
            .iter()
            .find(|change| change.path == path)
            .map(|change| change.status)
    }
}

/// Icon of the files extension with the number of changed files.
pub(super) struct FilesBadge {
    host: Host,
    model: Entity<ChangesModel>,
    _observe: Subscription,
}

impl FilesBadge {
    pub(super) fn new(host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = changes_model(host.clone(), window, cx);
        Self {
            host,
            _observe: cx.observe(&model, |_, _, cx| cx.notify()),
            model,
        }
    }
}

impl Render for FilesBadge {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let count = model.count().filter(|count| *count > 0);
        let mut details = Details::new("Файлы");
        details = match (model.count(), &model.changes) {
            (Some(0), _) => details.line("Изменений нет."),
            (Some(count), _) => details.line(format!("Изменённых файлов: {count}")),
            (None, Some(_)) => details.line("Это не git-репозиторий."),
            (None, None) => details,
        };
        let face = Rc::new(move || {
            icon_with_count(
                Icon::new(Lucide::FolderTree),
                count.map(|count| count.to_string()),
            )
            .into_any_element()
        });
        badge_frame(ID, face, details, &self.host, cx)
    }
}
