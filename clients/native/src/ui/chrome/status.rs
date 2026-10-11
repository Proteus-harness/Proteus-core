//! The bar under the tabs: panel toggles, what the agent is doing and the
//! extension icons placed there.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::preferences::BadgePlace;
use crate::ui::chat::ChatState;
use crate::ui::workspace::Workspace;
use crate::ui::panels;

impl Workspace {
    pub(in crate::ui) fn render_status_bar(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let layout = self.tab_layout(cx);
        let toggles = panels::available(cx)
            .into_iter()
            .enumerate()
            .map(|(index, panel)| {
                let shown = layout
                    .pane_of(&panel.id)
                    .is_some_and(|pane| layout.active(pane) == Some(panel.id.as_str()));
                let id = panel.id.clone();
                Button::new(("status-panel", index))
                    .ghost()
                    .xsmall()
                    .icon(panel.icon)
                    .tooltip(panel.title)
                    .selected(shown)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if shown {
                            this.close_tab(&id, window, cx);
                        } else {
                            this.open_panel(&id, None, window, cx);
                        }
                    }))
            });
        // Same height as the top bar.
        h_flex()
            .flex_none()
            .h(TITLE_BAR_HEIGHT)
            .px_1()
            .gap_1()
            .items_center()
            .bg(theme.status_bar)
            .border_t_1()
            .border_color(theme.status_bar_border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(h_flex().gap_0p5().children(toggles))
            .child(self.render_agent_state(cx))
            .child(div().flex_1())
            .child(self.render_badge_strip(BadgePlace::Status, cx))
    }

    /// State of the active chat, and how many others are working.
    fn render_agent_state(&self, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let state = |id: &String| self.chats.get(id).map(|chat| chat.view.read(cx).session.state());
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
                    .child(Spinner::new().xsmall())
                    .child(label),
            ),
            Some(ChatState::Trouble(label)) => Some(
                h_flex()
                    .gap_1p5()
                    .text_color(theme.warning)
                    .child(Icon::new(Lucide::TriangleAlert).xsmall())
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
