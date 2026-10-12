//! What the window shows: the folder picker until a folder is chosen, then
//! the workspace of that folder. Opening another folder from the top bar
//! replaces the workspace and its server.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{ButtonVariant, ButtonVariants as _};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::chrome::{self, Projects, WorkspaceRequest};
use super::prefs;
use super::workspace::{Status, Workspace};
use crate::launch::{LaunchOptions, Target};
use crate::ui::fade::faded;
use crate::ui::kit;

pub struct Launcher {
    stage: Stage,
    target: Target,
    focus: FocusHandle,
    _workspace_requests: Option<Subscription>,
}

enum Stage {
    Picking { error: Option<SharedString> },
    Ready(Entity<Workspace>),
}

impl Launcher {
    /// Starts the workspace right away when `options` name one.
    pub fn new(
        workspace: Option<PathBuf>,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let mut launcher = Self {
            stage: Stage::Picking { error: None },
            target,
            focus,
            _workspace_requests: None,
        };
        if let Some(workspace) = workspace {
            launcher.open(workspace, window, cx);
        }
        launcher
    }

    /// Shows `path` in the window; an open workspace and its server close.
    fn open(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = match path.canonicalize() {
            Ok(path) if path.is_dir() => path,
            _ => {
                let message = format!("Папка недоступна: {}", path.display());
                prefs::update(cx, |prefs| prefs.recent.forget(&path));
                match &mut self.stage {
                    Stage::Picking { error } => *error = Some(message.into()),
                    Stage::Ready(view) => view.update(cx, |this, cx| this.report(message, cx)),
                }
                cx.notify();
                return;
            }
        };
        if let Stage::Ready(view) = &self.stage {
            let current = view.read(cx);
            // A workspace whose server is gone opens again: that restarts it.
            let failed = matches!(current.status, Status::Failed(_));
            if current.workspace_path == workspace && !failed {
                return;
            }
            let running = if failed { 0 } else { current.running_chats(cx) };
            if running > 0 {
                self.confirm_switch(workspace, running, window, cx);
                return;
            }
        }
        self.replace(workspace, window, cx);
    }

    /// Asks before a folder switch ends runs that are still going.
    fn confirm_switch(
        &mut self,
        workspace: PathBuf,
        running: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let launcher = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (launcher, workspace) = (launcher.clone(), workspace.clone());
            alert
                .title("Открыть другую папку?")
                .description(format!(
                    "Агент ещё работает (чатов: {running}). Сервер этой папки закроется, \
                     и работа прервётся."
                ))
                .ok_text("Открыть")
                .ok_variant(ButtonVariant::Danger)
                .show_cancel(true)
                .cancel_text("Отмена")
                .on_ok(move |_, window, cx| {
                    launcher
                        .update(cx, |this, cx| this.replace(workspace.clone(), window, cx))
                        .ok();
                    true
                })
        });
    }

    fn replace(&mut self, workspace: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        prefs::update(cx, |prefs| prefs.recent.touch(&workspace));
        let options = LaunchOptions {
            workspace,
            target: self.target.clone(),
        };
        let view = cx.new(|cx| Workspace::new(options, window, cx));
        self._workspace_requests = Some(cx.subscribe_in(
            &view,
            window,
            |this, _, request: &WorkspaceRequest, window, cx| match request {
                WorkspaceRequest::Open(path) => this.open(path.clone(), window, cx),
                WorkspaceRequest::Choose => this.choose_folder(window, cx),
            },
        ));
        self.stage = Stage::Ready(view);
        cx.notify();
    }

    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Открыть".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.open(path, window, cx))
                .ok();
        })
        .detach();
    }

    fn render_picker(&self, error: Option<SharedString>, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let recent: Vec<PathBuf> = prefs::current(cx)
            .recent
            .iter()
            .map(Path::to_path_buf)
            .collect();
        let rows =
            recent.into_iter().enumerate().map(|(index, path)| {
                let exists = path.is_dir();
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                let shown = path.display().to_string();
                h_flex()
                    .id(("recent-workspace", index))
                    .px_3()
                    .py_2()
                    .gap_3()
                    .items_center()
                    .rounded(theme.radius)
                    .when(exists, |row| {
                        row.cursor_pointer()
                            .hover(|row| row.bg(theme.list_hover))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open(path.clone(), window, cx)
                            }))
                    })
                    .when(!exists, |row| row.opacity(0.5))
                    .child(
                        Icon::new(Lucide::Folder)
                            .small()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(div().text_sm().font_medium().child(faded(name)))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(
                                faded(if exists {
                                    shown
                                } else {
                                    format!("{shown} · папка не найдена")
                                }),
                            )),
                    )
            });
        let has_recent = prefs::current(cx).recent.iter().next().is_some();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .bg(theme.background)
            .child(
                v_flex()
                    .w(prefs::scaled(480.))
                    .gap_4()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_2xl().font_semibold().child("Proteus"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child("Выберите рабочую папку агента."),
                            ),
                    )
                    .children(
                        error.map(|error| div().text_sm().text_color(theme.danger).child(error)),
                    )
                    .when(has_recent, |column| {
                        column.child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .px_3()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(theme.muted_foreground)
                                        .child("Недавние"),
                                )
                                .children(rows),
                        )
                    })
                    .child(
                        h_flex().child(
                            kit::button("choose-workspace")
                                .icon(Lucide::FolderOpen)
                                .label("Открыть папку…")
                                .primary()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.choose_folder(window, cx)
                                })),
                        ),
                    ),
            )
            .into_any_element()
    }
}

impl Focusable for Launcher {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.stage {
            Stage::Ready(workspace) => div()
                .relative()
                .size_full()
                .child(workspace.clone())
                .child(super::wheel::layer())
                .child(super::tip::layer(cx))
                .into_any_element(),
            Stage::Picking { error } => {
                let launcher = cx.weak_entity();
                let chooser = launcher.clone();
                let projects = Projects::new(
                    None,
                    move |path, window, cx| {
                        launcher
                            .update(cx, |this, cx| this.open(path, window, cx))
                            .ok();
                    },
                    move |window, cx| {
                        chooser
                            .update(cx, |this, cx| this.choose_folder(window, cx))
                            .ok();
                    },
                );
                v_flex()
                    .relative()
                    .size_full()
                    .track_focus(&self.focus)
                    .child(chrome::picker_top_bar(projects, window, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(self.render_picker(error.clone(), cx)),
                    )
                    .child(super::wheel::layer())
                    .child(super::tip::layer(cx))
                    .into_any_element()
            }
        }
    }
}
