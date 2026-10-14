//! The header every panel starts with: its title, then the panel's own
//! actions and the button that hides the side panel, at the same height
//! and padding for all panels.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::PanelInfo;
use crate::preferences::SESSIONS_PANEL;
use crate::shortcuts::Command;
use crate::ui::fade::faded;
use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::workspace::Workspace;

impl Workspace {
    pub(super) fn render_panel_header(
        &self,
        panel: &PanelInfo,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let name = panel.name;
        h_flex()
            .flex_none()
            .h_10()
            .pl_4()
            .pr_2()
            .gap_1()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .font_semibold()
                    .child(faded(panel.title)),
            )
            .when(name == SESSIONS_PANEL, |header| {
                header.child(
                    kit::button("new-session")
                        .icon(IconName::Plus)
                        .ghost()
                        .small()
                        .hint_keys(
                            "Новая сессия",
                            kit::command_keys(Command::NewSession, cx).as_deref(),
                            cx,
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.new_session(None, window, cx)),
                        ),
                )
            })
            .child(
                kit::button("panel-hide")
                    .icon(Lucide::PanelLeftClose)
                    .ghost()
                    .small()
                    .hint("Скрыть панель", cx)
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.press_panel(name, window, cx)),
                    ),
            )
    }
}
