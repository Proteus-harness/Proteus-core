//! The bar under the tabs: what the agent is doing and the extension icons
//! placed at either end.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::preferences::BadgePlace;
use crate::ui::chat::ChatState;
use crate::ui::motion::Spinner;
use crate::ui::prefs;
use crate::ui::workspace::Workspace;

impl Workspace {
    pub(in crate::ui) fn render_status_bar(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        // Same height as the top bar.
        h_flex()
            .flex_none()
            .h(prefs::scaled(super::BAR_HEIGHT))
            .px_1()
            .gap_1()
            .items_center()
            .bg(theme.status_bar)
            .border_t_1()
            .border_color(theme.status_bar_border)
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(self.render_badge_strip(BadgePlace::StatusStart, cx))
            .child(self.render_agent_state(cx))
            .child(div().flex_1())
            .child(self.render_badge_strip(BadgePlace::StatusEnd, cx))
    }

    /// State of the active chat, and how many others are working.
    fn render_agent_state(&self, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let state = |id: &String| {
            self.chats
                .get(id)
                .map(|chat| chat.view.read(cx).session.state())
        };
        let active = self.active_chat.as_ref().and_then(state);
        let others = self
            .chats
            .keys()
            .filter(|id| Some(*id) != self.active_chat.as_ref())
            .filter(|id| matches!(state(id), Some(ChatState::Busy(_))))
            .count();
        let current = match active {
            Some(ChatState::Busy(label)) => Some(
                h_flex()
                    .gap_1p5()
                    .child(Spinner::new().small())
                    .child(label),
            ),
            Some(ChatState::Trouble(label)) => Some(
                h_flex()
                    .gap_1p5()
                    .text_color(theme.warning)
                    .child(Icon::new(Lucide::TriangleAlert).small())
                    .child(label),
            ),
            Some(ChatState::Idle) | None => None,
        };
        h_flex()
            .pl_2()
            .gap_3()
            .items_center()
            .children(current)
            .when(others > 0, |row| {
                row.child(format!("ещё в работе: {others}"))
            })
    }
}
