//! The session list panel: sessions of the workspace and a new-session
//! button.

use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppSessionActivityStatus;

use super::session_menu;
use crate::sessions::Row;
use crate::shortcuts::Command;
use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::motion::Spinner;
use crate::ui::workspace::Workspace;
use crate::ui::{format, motion, prefs};

/// How long after the list arrives its rows still enter one by one.
const ENTRANCE: Duration = Duration::from_millis(600);

impl Workspace {
    pub(super) fn render_sessions(&self, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let current = self.active_chat_dir();
        let workspace_name = self
            .workspace_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.workspace_path.display().to_string());
        let marks = &prefs::current(cx).sessions;
        let listing = marks.arrange(&self.sessions, self.show_archived);
        let mut items: Vec<AnyElement> = Vec::new();
        let mut ix = 0;
        let entering = self
            .sessions_arrived
            .is_some_and(|at| at.elapsed() < ENTRANCE);
        let mut rows = |rows: &[Row], items: &mut Vec<AnyElement>| {
            for row in rows {
                let selected = current == Some(row.summary.session_dir.as_path());
                let id = format!("session-enter-{}", row.summary.session_dir.display());
                items.push(
                    motion::stagger_in(
                        div().child(self.render_session_item(ix, row, selected, cx)),
                        ElementId::Name(id.into()),
                        ix,
                        entering,
                    )
                    .into_any_element(),
                );
                ix += 1;
            }
        };
        if !listing.pinned.is_empty() {
            items.push(heading("Закреплённые", cx).into_any_element());
            rows(&listing.pinned, &mut items);
            if !listing.recent.is_empty() {
                items.push(heading("Остальные", cx).into_any_element());
            }
        }
        rows(&listing.recent, &mut items);
        if listing.archived_count > 0 {
            items.push(
                self.render_archive_toggle(listing.archived_count, cx)
                    .into_any_element(),
            );
            rows(&listing.archived, &mut items);
        }
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .pl_4()
                    .pr_2()
                    .py_1()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .font_semibold()
                            .truncate()
                            .text_color(theme.muted_foreground)
                            .child(workspace_name),
                    )
                    .child(
                        kit::button("new-session")
                            .icon(IconName::Plus)
                            .ghost()
                            .small()
                            .hint_keys(
                                "Новая сессия",
                                kit::command_keys(Command::NewSession, cx).as_deref(),
                                cx,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_session(None, window, cx)
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("sessions")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_2()
                    .gap_0p5()
                    .children(items),
            )
    }

    fn render_archive_toggle(&self, count: usize, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .id("sessions-archive")
            .mt_2()
            .px_2()
            .py_1()
            .gap_1()
            .rounded(theme.radius)
            .text_xs()
            .text_color(theme.muted_foreground)
            .cursor_pointer()
            .hover(|row| row.bg(theme.list_hover))
            .child(motion::chevron(
                "sessions-archive-chevron",
                self.show_archived,
                theme.muted_foreground,
            ))
            .child(format!("Архив · {count}"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.show_archived = !this.show_archived;
                cx.notify();
            }))
    }

    fn render_session_item(
        &self,
        ix: usize,
        row: &Row,
        selected: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let summary = row.summary;
        let mark = row.mark.clone();
        let dir = summary.session_dir.clone();
        let renaming = self
            .renaming
            .as_ref()
            .filter(|renaming| renaming.dir == dir)
            .map(|renaming| renaming.input.clone());
        let status = summary.activity.as_ref().map(|activity| activity.status);
        let indicator = match status {
            Some(AppSessionActivityStatus::Running) => {
                Some(Spinner::new().xsmall().into_any_element())
            }
            Some(
                AppSessionActivityStatus::WaitingApproval | AppSessionActivityStatus::WaitingInput,
            ) => Some(
                Icon::new(IconName::CircleAlert)
                    .xsmall()
                    .text_color(theme.warning)
                    .into_any_element(),
            ),
            _ => None,
        };

        let workspace = cx.entity().downgrade();
        h_flex()
            .id(("session", ix))
            .group("session-row")
            .relative()
            .px_2()
            .py_1p5()
            .gap_2()
            .rounded(theme.radius)
            .cursor_pointer()
            .when(selected, |row| {
                row.bg(theme.sidebar_accent)
                    .text_color(theme.sidebar_accent_foreground)
            })
            .when(!selected, |row| row.hover(|row| row.bg(theme.list_hover)))
            .when(mark.archived && !selected, |row| {
                row.text_color(theme.muted_foreground)
            })
            .when(mark.pinned, |row| {
                row.child(
                    Icon::new(Lucide::Pin)
                        .xsmall()
                        .flex_none()
                        .text_color(theme.muted_foreground),
                )
            })
            .child(match renaming {
                Some(input) => div()
                    .flex_1()
                    .min_w_0()
                    .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key == "escape" {
                            cx.stop_propagation();
                            this.finish_rename(false, cx);
                        }
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(Input::new(&input).xsmall())
                    .into_any_element(),
                None => div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .truncate()
                    .child(
                        mark.title
                            .clone()
                            .unwrap_or_else(|| format::session_title(summary)),
                    )
                    .into_any_element(),
            })
            .children(indicator)
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .group_hover("session-row", |time| time.invisible())
                    .child(format::relative_time(summary.updated_at_ms)),
            )
            .child(
                div()
                    .absolute()
                    .right_1()
                    .invisible()
                    .group_hover("session-row", |menu| menu.visible())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        kit::button(("session-menu", ix))
                            .icon(Lucide::Ellipsis)
                            .ghost()
                            .xsmall()
                            .dropdown_menu_with_anchor(Anchor::TopRight, {
                                let (dir, mark) = (dir.clone(), mark.clone());
                                let workspace = workspace.clone();
                                move |menu, _, _| {
                                    session_menu::build(menu, &dir, mark.clone(), &workspace)
                                }
                            }),
                    ),
            )
            .on_click(cx.listener({
                let dir = dir.clone();
                move |this, event: &ClickEvent, window, cx| {
                    this.select_session(dir.clone(), event.click_count(), window, cx)
                }
            }))
            .context_menu(move |menu, _, _| {
                session_menu::build(menu, &dir, mark.clone(), &workspace)
            })
    }
}

fn heading(label: &'static str, cx: &App) -> impl IntoElement {
    div()
        .px_2()
        .pt_2()
        .pb_0p5()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(label)
}
