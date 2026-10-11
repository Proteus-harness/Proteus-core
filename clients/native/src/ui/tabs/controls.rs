//! Buttons at the end of a pane's tab bar: splitting and closing the pane,
//! and extension icons in the top-right pane.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::preferences::BadgePlace;
use crate::tabs::{Edge, Pane, TabLayout};
use crate::ui::workspace::Workspace;

impl Workspace {
    pub(super) fn render_pane_controls(
        &self,
        pane: &Pane,
        layout: &TabLayout,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let id = pane.id;
        let key = id.0 as usize;
        let can_split = pane.tabs.len() > 1;
        let corner = layout
            .rect(id)
            .is_some_and(|rect| rect.y <= 1e-4 && rect.x + rect.w >= 1. - 1e-4);
        let split = |name: &'static str, icon: Lucide, tooltip: &'static str, edge: Edge| {
            Button::new((name, key))
                .ghost()
                .xsmall()
                .icon(Icon::new(icon))
                .disabled(!can_split)
                .tooltip(tooltip)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(tab) = this.tab_layout(cx).active(id).map(str::to_string) {
                        this.split_tab(&tab, id, edge, window, cx);
                    }
                }))
        };
        h_flex()
            .flex_none()
            .h_full()
            .px_1()
            .gap_1()
            .child(split(
                "pane-split-right",
                Lucide::Columns2,
                "Разделить вправо",
                Edge::Right,
            ))
            .child(split(
                "pane-split-down",
                Lucide::Rows2,
                "Разделить вниз",
                Edge::Bottom,
            ))
            .when(layout.pane_count() > 1, |controls| {
                controls.child(
                    Button::new(("pane-close", key))
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(Lucide::SquareX))
                        .tooltip("Закрыть область, вкладки перейдут в соседнюю")
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.close_pane(id, window, cx)),
                        ),
                )
            })
            .when(corner, |controls| {
                controls.child(
                    h_flex()
                        .h_full()
                        .pl_2()
                        .gap_1()
                        .border_l_1()
                        .border_color(theme.border)
                        .child(self.render_badge_strip(BadgePlace::Header, cx)),
                )
            })
    }
}
