//! The tab strip of a pane: its tabs, pinned ones first, the drop place
//! at its end and the pane controls.

use std::time::Instant;

use gpui_kit::component::*;
use gpui_kit::*;

use super::tab::{BAR_HEIGHT, DraggedTab, TabState};
use crate::tabs::{Pane, TabLayout};
use crate::ui::drag;
use crate::ui::motion::Collapse;
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
                    .children(self.render_strip_tabs(pane, layout, cx))
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

    /// The tabs of a strip, each in a reveal that lets it grow in when it
    /// opens; closed tabs linger where they were while they shrink away.
    fn render_strip_tabs(
        &self,
        pane: &Pane,
        layout: &TabLayout,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let now = Instant::now();
        let focused = layout.focused == pane.id;
        let mut tabs = pane
            .tabs
            .iter()
            .enumerate()
            .map(|(ix, tab)| {
                let state = TabState {
                    active: pane.active.as_deref() == Some(tab.as_str()),
                    focused,
                    preview: layout.preview.as_deref() == Some(tab.as_str()),
                    pinned: ix < pane.pinned,
                };
                let shown = self.tab_motion.entering(tab, now).unwrap_or(1.);
                let element = self.render_tab(pane.id, tab, state, layout, cx);
                reveal(tab, shown, element.into_any_element())
            })
            .collect::<Vec<_>>();
        for (leaving, shown) in self.tab_motion.leaving(pane.id, now) {
            let state = TabState {
                active: false,
                focused: false,
                preview: false,
                pinned: leaving.pinned,
            };
            let element = self.render_tab(pane.id, &leaving.id, state, layout, cx);
            let ix = leaving.index.min(tabs.len());
            tabs.insert(ix, reveal(&leaving.id, shown, element.into_any_element()));
        }
        tabs
    }
}

/// A tab clipped to `shown` of its width and as transparent; the same id
/// before and after it closes keeps its measured width.
fn reveal(id: &str, shown: f32, tab: AnyElement) -> AnyElement {
    let id = ElementId::Name(format!("tab-reveal-{id}").into());
    if shown >= 1. {
        return Collapse::new(id, 1., tab).into_any_element();
    }
    Collapse::new(
        id,
        shown,
        div().h_full().opacity(shown).child(tab).into_any_element(),
    )
    .into_any_element()
}
