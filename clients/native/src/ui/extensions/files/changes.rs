use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{FilesPage, ID, Mode, path_label, status_color};
use crate::files::Changes;
use crate::ui::extensions::badge::{Details, badge_frame, icon_with_count};
use crate::ui::extensions::frame::{is_open, muted, widget_frame};
use crate::ui::extensions::shared::shared;
use crate::ui::extensions::{Host, HostEvent};

const VISIBLE: usize = 12;

/// Git changes of the workspace, shared by the panel widget and the icon.
struct ChangesModel {
    host: Host,
    changes: Option<Changes>,
    error: Option<SharedString>,
    loading: bool,
    _events: Option<Subscription>,
}

fn model(host: Host, window: &mut Window, cx: &mut App) -> Entity<ChangesModel> {
    shared(cx, |cx| {
        cx.new(|cx| {
            let events = host.subscribe(cx, |this: &mut ChangesModel, event, cx| match event {
                HostEvent::SessionOpened | HostEvent::RunSettled | HostEvent::PanelOpened => {
                    this.refresh(cx)
                }
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
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let (Some(api), Some(dir)) = (self.host.api(cx), self.host.session_dir(cx)) else {
            return;
        };
        self.loading = true;
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

    /// Number of changed files, when the workspace is a repository.
    fn count(&self) -> Option<usize> {
        self.changes
            .as_ref()
            .filter(|changes| changes.repository)
            .map(|changes| changes.entries.len())
    }
}

/// Opens the «Файлы» page, revealing the diff of `path` when given.
fn open(host: &Host, path: Option<String>, window: &mut Window, cx: &mut App) {
    let Some(view) = host.open_page(ID, window, cx) else {
        return;
    };
    if let (Some(path), Ok(page)) = (path, view.downcast::<FilesPage>()) {
        page.update(cx, |page, cx| page.reveal(&path, Mode::Diff, window, cx));
    }
}

/// Panel widget with the git changes of the workspace; a click opens the
/// file diff on the «Файлы» page.
pub(super) struct ChangesWidget {
    host: Host,
    model: Entity<ChangesModel>,
    _observe: Subscription,
}

impl ChangesWidget {
    pub(super) fn new(host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = model(host.clone(), window, cx);
        Self {
            host,
            _observe: cx.observe(&model, |_, _, cx| cx.notify()),
            model,
        }
    }

    fn body(&self, cx: &mut Context<Self>) -> AnyElement {
        let model = self.model.read(cx);
        if let Some(error) = &model.error {
            return muted(format!("Не удалось получить изменения: {error}"), cx).into_any_element();
        }
        let Some(changes) = &model.changes else {
            return muted("Загрузка…", cx).into_any_element();
        };
        if !changes.repository {
            return muted("Это не git-репозиторий.", cx).into_any_element();
        }
        let theme = cx.theme();
        let rows = changes
            .entries
            .iter()
            .take(VISIBLE)
            .enumerate()
            .map(|(index, change)| {
                let path = change.path.clone();
                let host = self.host.clone();
                h_flex()
                    .id(("changes-widget-row", index))
                    .h_6()
                    .px_1()
                    .gap_2()
                    .text_sm()
                    .rounded(theme.radius)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.list_hover))
                    .child(
                        div()
                            .w_3()
                            .flex_none()
                            .text_xs()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(status_color(change.status, cx))
                            .child(change.status.mark()),
                    )
                    .child(path_label(&change.path, cx).flex_1())
                    .on_click(move |_, window, cx| open(&host, Some(path.clone()), window, cx))
            })
            .collect::<Vec<_>>();
        let hidden = changes.entries.len().saturating_sub(VISIBLE);
        let host = self.host.clone();
        v_flex()
            .gap_0p5()
            .when(changes.entries.is_empty(), |list| {
                list.child(muted("Изменений нет.", cx))
            })
            .children(rows)
            .when(hidden > 0, |list| {
                list.child(muted(format!("и ещё {hidden}"), cx).px_1())
            })
            .child(
                h_flex().pt_1().child(
                    Button::new("changes-open-files")
                        .label("Открыть файлы")
                        .ghost()
                        .xsmall()
                        .on_click(move |_, window, cx| open(&host, None, window, cx)),
                ),
            )
            .into_any_element()
    }
}

impl Render for ChangesWidget {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self
            .model
            .read(cx)
            .count()
            .filter(|count| *count > 0)
            .map(|count| SharedString::from(count.to_string()));
        let body = is_open(ID, cx).then(|| self.body(cx));
        widget_frame(ID, "Изменения".into(), summary, body, cx)
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
        let model = model(host.clone(), window, cx);
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
        let face = icon_with_count(
            Icon::new(Lucide::FolderTree),
            count.map(|count| count.to_string()),
        );
        badge_frame(ID, face, details, &self.host, cx)
    }
}
