use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::{AppSessionActivityStatus, AppSessionSummary};

use super::format;
use super::workspace::Workspace;

pub(super) const SIDEBAR_WIDTH: f32 = 264.;

impl Workspace {
    pub(super) fn render_sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let current = self.session.as_ref().map(|session| session.dir.clone());
        let workspace_name = self
            .workspace_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.workspace_path.display().to_string());

        let items = self.sessions.iter().enumerate().map(|(ix, summary)| {
            let selected = current.as_ref() == Some(&summary.session_dir);
            self.render_session_item(ix, summary, selected, cx)
        });

        v_flex()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .truncate()
                            .child(workspace_name),
                    )
                    .child(
                        Button::new("new-session")
                            .icon(IconName::Plus)
                            .ghost()
                            .small()
                            .tooltip("Новая сессия")
                            .on_click(cx.listener(|this, _, _, cx| this.new_session(cx))),
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

    fn render_session_item(
        &self,
        ix: usize,
        summary: &AppSessionSummary,
        selected: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let dir = summary.session_dir.clone();
        let status = summary
            .activity
            .as_ref()
            .map(|activity| activity.status.clone());
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
            .on_click(cx.listener(move |this, _, _, cx| this.select_session(dir.clone(), cx)))
    }
}
