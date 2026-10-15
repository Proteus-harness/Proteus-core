//! The session list panel: the workspace's sessions grouped by when they
//! changed, with a filter by title.

use std::path::Path;
use std::time::Duration;

use chrono::Local;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppSessionActivityStatus;

use super::session_menu;
use crate::sessions::{Row, View};
use crate::ui::fade::faded;
use crate::ui::kit;
use crate::ui::motion::Spinner;
use crate::ui::workspace::Workspace;
use crate::ui::{format, motion, prefs};

/// How long after the list arrives its rows still enter one by one.
const ENTRANCE: Duration = Duration::from_millis(600);

impl Workspace {
    pub(super) fn render_sessions(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        let current = self.active_chat_dir().map(Path::to_path_buf);
        let cursor = (self.session_list.focus.is_focused(window))
            .then(|| self.session_cursor())
            .flatten();
        let searching = !self.session_list.search.is_empty();
        let view = View {
            archived: self.show_archived,
            search: &self.session_list.search,
            today: Local::now().date_naive(),
        };
        let listing = prefs::current(cx).sessions.arrange(&self.sessions, &view);
        let mut items: Vec<AnyElement> = Vec::new();
        let mut shown = Vec::new();
        let entering = self
            .sessions_arrived
            .is_some_and(|at| at.elapsed() < ENTRANCE);
        let mut rows = |rows: &[Row], items: &mut Vec<AnyElement>| {
            for row in rows {
                let dir = &row.summary.session_dir;
                let ix = shown.len();
                let look = Look {
                    selected: current.as_ref() == Some(dir),
                    cursor: cursor.as_ref() == Some(dir),
                };
                let id = format!("session-enter-{}", dir.display());
                shown.push((dir.clone(), items.len()));
                items.push(
                    motion::stagger_in(
                        div().child(self.render_session_item(ix, row, look, cx)),
                        ElementId::Name(id.into()),
                        ix,
                        entering,
                    )
                    .into_any_element(),
                );
            }
        };
        if !listing.pinned.is_empty() {
            items.push(heading("Закреплённые", cx).into_any_element());
            rows(&listing.pinned, &mut items);
        }
        for group in &listing.recent {
            items.push(heading(group.period.label(), cx).into_any_element());
            rows(&group.rows, &mut items);
        }
        if searching {
            if !listing.archived.is_empty() {
                items.push(heading("Архив", cx).into_any_element());
                rows(&listing.archived, &mut items);
            }
        } else if listing.archived_count > 0 {
            items.push(
                self.render_archive_toggle(listing.archived_count, cx)
                    .into_any_element(),
            );
            rows(&listing.archived, &mut items);
        }
        if searching && shown.is_empty() {
            items.push(
                div()
                    .px_2()
                    .py_2()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Ничего не найдено")
                    .into_any_element(),
            );
        }
        self.session_list.shown = shown;
        let list = v_flex()
            .id("sessions")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.session_list.scroll)
            .px_2()
            .pb_2()
            .gap_0p5()
            .children(items);
        let list = self
            .session_list_actions(v_flex().flex_1().min_h_0(), cx)
            .child(list);
        let filter = div().flex_none().px_2().pb_2().child(
            Input::new(&self.session_list.filter)
                .small()
                .cleanable(true)
                .prefix(Icon::new(IconName::Search).small()),
        );
        self.session_panel_actions(v_flex().size_full(), cx)
            .child(filter)
            .child(list)
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
        look: Look,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let Look { selected, cursor } = look;
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
            .when(!selected && cursor, |row| row.bg(theme.list_hover))
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
                    .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            cx.stop_propagation();
                            this.finish_rename(false, cx);
                            this.session_list.focus.focus(window, cx);
                        }
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(Input::new(&input).xsmall())
                    .into_any_element(),
                None => div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .child(faded(
                        mark.title
                            .clone()
                            .unwrap_or_else(|| format::session_title(summary)),
                    ))
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
                    this.session_list.cursor = Some(dir.clone());
                    this.session_list.focus.focus(window, cx);
                    this.select_session(dir.clone(), event.click_count(), window, cx)
                }
            }))
            .context_menu(move |menu, _, _| {
                session_menu::build(menu, &dir, mark.clone(), &workspace)
            })
    }
}

/// How a row stands out: the open session, the keyboard cursor.
#[derive(Clone, Copy)]
struct Look {
    selected: bool,
    cursor: bool,
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
