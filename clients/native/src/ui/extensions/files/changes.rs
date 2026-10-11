use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{FilesPage, ID, Mode, path_label, status_color};
use crate::files::Changes;
use crate::ui::extensions::frame::{is_open, muted, widget_frame};
use crate::ui::extensions::{Host, HostEvent};

const VISIBLE: usize = 12;

/// Panel widget with the git changes of the workspace; a click opens the
/// file diff on the «Файлы» page.
pub(super) struct ChangesWidget {
    host: Host,
    changes: Option<Changes>,
    error: Option<SharedString>,
    loading: bool,
    _events: Option<Subscription>,
}

impl ChangesWidget {
    pub(super) fn new(host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let events = host.subscribe(cx, |this, event, cx| match event {
            HostEvent::SessionOpened | HostEvent::RunSettled | HostEvent::PanelOpened => {
                this.refresh(cx)
            }
        });
        cx.defer_in(window, |this, _, cx| this.refresh(cx));
        Self {
            host,
            changes: None,
            error: None,
            loading: false,
            _events: events,
        }
    }

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

    fn open(&mut self, path: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.host.open_page(ID, window, cx) else {
            return;
        };
        if let (Some(path), Ok(page)) = (path, view.downcast::<FilesPage>()) {
            page.update(cx, |page, cx| page.reveal(&path, Mode::Diff, window, cx));
        }
    }

    fn body(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(error) = &self.error {
            return muted(format!("Не удалось получить изменения: {error}"), cx).into_any_element();
        }
        let Some(changes) = &self.changes else {
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
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open(Some(path.clone()), window, cx)
                    }))
            })
            .collect::<Vec<_>>();
        let hidden = changes.entries.len().saturating_sub(VISIBLE);
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
                        .on_click(cx.listener(|this, _, window, cx| this.open(None, window, cx))),
                ),
            )
            .into_any_element()
    }
}

impl Render for ChangesWidget {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self
            .changes
            .as_ref()
            .filter(|changes| changes.repository && !changes.entries.is_empty())
            .map(|changes| SharedString::from(changes.entries.len().to_string()));
        let body = is_open(ID, cx).then(|| self.body(cx));
        widget_frame(ID, "Изменения".into(), summary, body, cx)
    }
}
