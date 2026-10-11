//! The session list panel: sessions of the workspace, a new-session button
//! and the way to the settings.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::{AppSessionActivityStatus, AppSessionSummary};

use crate::ui::format;
use crate::ui::workspace::Workspace;

impl Workspace {
    pub(super) fn render_sessions(&self, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let current = self.active_chat_dir();
        let workspace_name = self
            .workspace_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.workspace_path.display().to_string());
        let items = self.sessions.iter().enumerate().map(|(ix, summary)| {
            let selected = current == Some(summary.session_dir.as_path());
            self.render_session_item(ix, summary, selected, cx)
        });
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
                        Button::new("new-session")
                            .icon(IconName::Plus)
                            .ghost()
                            .small()
                            .tooltip("Новая сессия (Ctrl+N)")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.new_session(window, cx)),
                            ),
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
            .child(
                h_flex()
                    .flex_none()
                    .px_2()
                    .py_2()
                    .border_t_1()
                    .border_color(theme.sidebar_border)
                    .child(
                        h_flex()
                            .id("open-settings")
                            .w_full()
                            .h_8()
                            .px_2()
                            .gap_2()
                            .rounded(theme.radius)
                            .text_sm()
                            .text_color(theme.sidebar_foreground)
                            .cursor_pointer()
                            .hover(|item| item.bg(theme.list_hover))
                            .child(
                                Icon::new(IconName::Settings)
                                    .small()
                                    .text_color(theme.muted_foreground),
                            )
                            .child("Настройки")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_settings(window, cx)),
                            ),
                    ),
            )
    }

    fn render_session_item(
        &self,
        ix: usize,
        summary: &AppSessionSummary,
        selected: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let dir = summary.session_dir.clone();
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

        h_flex()
            .id(("session", ix))
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
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .truncate()
                    .child(format::session_title(summary)),
            )
            .children(indicator)
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format::relative_time(summary.updated_at_ms)),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.select_session(dir.clone(), event.click_count(), window, cx)
            }))
    }
}
