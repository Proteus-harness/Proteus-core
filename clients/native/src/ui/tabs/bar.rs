//! The tab strip of a pane: its tabs, pinned ones first, the drop place
//! at its end and the pane controls.

use gpui_kit::component::*;
use gpui_kit::*;

use super::tab::{BAR_HEIGHT, DraggedTab, TabState};
use crate::tabs::{Pane, TabLayout};
use crate::ui::drag;
use crate::ui::workspace::Workspace;

impl Workspace {
    pub(super) fn render_tab_bar(
        &self,
        pane: &Pane,
        layout: &TabLayout,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let id = pane.id;
        let focused = layout.focused == id;
        let end_group = SharedString::from(format!("tab-end-{}", id.0));
        let strip = self
            .tab_strips
            .get(&id)
            .map(|(strip, _)| strip.clone())
            .unwrap_or_default();
        h_flex()
            .h(px(BAR_HEIGHT))
            .flex_none()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .id(("tab-strip", id.0 as usize))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_x_scroll()
                    .track_scroll(&strip)
                    .children(pane.tabs.iter().enumerate().map(|(ix, tab)| {
                        let state = TabState {
                            active: pane.active.as_deref() == Some(tab.as_str()),
                            focused,
                            preview: layout.preview.as_deref() == Some(tab.as_str()),
                            pinned: ix < pane.pinned,
                        };
                        self.render_tab(id, tab, state, layout, cx)
                    }))
                    .child(
                        div()
                            .id(("tab-strip-end", id.0 as usize))
                            .group(end_group.clone())
                            .relative()
                            .flex_1()
                            .min_w(px(24.))
                            .h_full()
                            .child(drag::insertion_mark::<DraggedTab>(end_group, cx))
                            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                                this.tab_drag = false;
                                this.move_tab(&dragged.id, id, None, window, cx);
                            })),
                    ),
            )
            .child(self.render_pane_controls(pane, layout, cx))
    }
}
