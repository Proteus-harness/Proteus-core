//! The rail along the left edge, under the top bar: what the agent is doing
//! and the narrow extension icons placed at either end.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::*;
use gpui_kit::*;

use crate::preferences::BadgePlace;
use crate::ui::chat::ChatState;
use crate::ui::motion::Spinner;
use crate::ui::prefs;
use crate::ui::tip::Tip as _;
use crate::ui::workspace::Workspace;

/// Rail width before zoom.
const RAIL_WIDTH: f32 = 40.;

impl Workspace {
    pub(in crate::ui) fn render_rail(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        v_flex()
            .flex_none()
            .w(prefs::scaled(RAIL_WIDTH))
            .h_full()
            .py_1()
            .gap_1()
            .items_center()
            .bg(theme.status_bar)
            .border_r_1()
            .border_color(theme.status_bar_border)
            .text_color(theme.muted_foreground)
            .child(self.render_agent_state(cx))
            .child(self.render_badge_strip(BadgePlace::RailStart, cx))
            .child(div().flex_1())
            .child(self.render_badge_strip(BadgePlace::RailEnd, cx))
    }

    /// State of the active chat, and how many others are working; the words
    /// are in the tooltips.
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
            Some(ChatState::Busy(label)) => Some(cell(
                "rail-agent-state",
                label,
                Spinner::new().small().into_any_element(),
            )),
            Some(ChatState::Trouble(label)) => Some(cell(
                "rail-agent-state",
                label,
                Icon::new(Lucide::TriangleAlert)
                    .small()
                    .text_color(theme.warning)
                    .into_any_element(),
            )),
            Some(ChatState::Idle) | None => None,
        };
        let others = (others > 0).then(|| {
            cell(
                "rail-agent-others",
                format!("ещё в работе: {others}"),
                div()
                    .text_xs()
                    .child(format!("+{others}"))
                    .into_any_element(),
            )
        });
        v_flex()
            .gap_1()
            .items_center()
            .children(current)
            .children(others)
    }
}

/// An icon-sized cell that names what it shows in a tooltip.
fn cell(
    id: &'static str,
    tip: impl Into<SharedString>,
    content: AnyElement,
) -> impl IntoElement {
    div()
        .id(id)
        .size_7()
        .flex()
        .items_center()
        .justify_center()
        .child(content)
        .tip_text(tip)
}
