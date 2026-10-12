//! The tab strip of a pane: its tabs, pinned ones first, and the pane
//! controls. While a tab is dragged the strip makes room for it where it
//! would land.

use std::time::Instant;

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::arrange::{self, Frame};
use super::tab::{BAR_HEIGHT, DraggedTab, TabState};
use crate::tabs::{Pane, TabLayout};
use crate::ui::motion::{BASE, Collapse, glide};
use crate::ui::prefs;
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
        let strip = self
            .tab_strips
            .get(&id)
            .map(|(strip, _)| strip.clone())
            .unwrap_or_default();
        let frame = arrange::frame(pane, layout, cx);
        let tabs = match &frame.dragging {
            Some(dragging) => self.render_dragged_strip(pane, layout, &frame, dragging, cx),
            None => self.render_strip_tabs(pane, layout, cx),
        };
        h_flex()
            .relative()
            .h(prefs::scaled(BAR_HEIGHT))
            .flex_none()
            .bg(theme.tab_bar)
            // The divider is drawn under the tabs, so the active tab opens
            // into its content.
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(1.))
                    .bg(theme.border),
            )
            .child(
                h_flex()
                    .id(("tab-strip", id.0 as usize))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_x_scroll()
                    .track_scroll(&strip)
                    .children(tabs)
                    // The free end of the strip takes drops too.
                    .child(div().flex_1().min_w(prefs::scaled(24.)).h_full())
                    .when_some(frame.dragging, |strip, dragging| {
                        let pinned = dragged_state(&dragging, layout).is_some_and(|s| s.pinned);
                        let (hovered, dropped) = (pane.clone(), pane.clone());
                        strip
                            .on_drag_move::<DraggedTab>(move |event, window, _| {
                                arrange::hover(
                                    &hovered,
                                    &dragging,
                                    pinned,
                                    event.bounds,
                                    event.event.position,
                                    window,
                                )
                            })
                            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                                this.tab_drag = false;
                                if let Some(before) = arrange::drop_before(&dropped, &dragged.id) {
                                    let before = before.as_deref();
                                    this.move_tab(&dragged.id, dropped.id, before, window, cx);
                                }
                            }))
                    }),
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
        let mut tabs = pane
            .tabs
            .iter()
            .enumerate()
            .map(|(ix, tab)| {
                let shown = self.tab_motion.entering(tab, now).unwrap_or(1.);
                self.strip_tab(pane, tab, tab_state(pane, ix, layout), shown, layout, cx)
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

    /// The other tabs of a strip with a gap before, between and after them;
    /// the open gap holds the dragged tab itself, the others are shut.
    fn render_dragged_strip(
        &self,
        pane: &Pane,
        layout: &TabLayout,
        frame: &Frame,
        dragging: &str,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let mut dragged = frame
            .open_gap
            .and(dragged_state(dragging, layout))
            .map(|state| {
                self.render_tab(pane.id, dragging, state, layout, cx)
                    .into_any_element()
            });
        let mut gap = |index: usize| {
            let open = frame.open_gap == Some(index);
            TabGap {
                id: SharedString::from(format!(
                    "tab-gap-{}-{index}-{}",
                    pane.id.0, frame.generation
                ))
                .into(),
                open,
                width: frame.gap_width,
                tab: if open { dragged.take() } else { None },
            }
            .into_any_element()
        };
        let mut row = Vec::new();
        let mut index = 0;
        for (ix, tab) in pane.tabs.iter().enumerate() {
            if tab == dragging {
                continue;
            }
            row.push(gap(index));
            row.push(self.strip_tab(pane, tab, tab_state(pane, ix, layout), 1., layout, cx));
            index += 1;
        }
        row.push(gap(index));
        row
    }

    /// A tab of the strip that reports where it is drawn.
    fn strip_tab(
        &self,
        pane: &Pane,
        tab: &str,
        state: TabState,
        shown: f32,
        layout: &TabLayout,
        cx: &Context<Self>,
    ) -> AnyElement {
        let element = self.render_tab(pane.id, tab, state, layout, cx);
        let id = tab.to_string();
        let measured = div()
            .flex_none()
            .h_full()
            .on_prepaint(move |bounds, _, _| arrange::record(&id, bounds))
            .child(element);
        reveal(tab, shown, measured.into_any_element())
    }
}

fn tab_state(pane: &Pane, ix: usize, layout: &TabLayout) -> TabState {
    let tab = pane.tabs[ix].as_str();
    TabState {
        active: pane.active.as_deref() == Some(tab),
        focused: layout.focused == pane.id,
        preview: layout.preview.as_deref() == Some(tab),
        pinned: ix < pane.pinned,
    }
}

/// How the dragged tab looks: as it is in its own pane.
fn dragged_state(dragging: &str, layout: &TabLayout) -> Option<TabState> {
    let home = layout.pane(layout.pane_of(dragging)?)?;
    let ix = home.tabs.iter().position(|tab| tab == dragging)?;
    Some(tab_state(home, ix, layout))
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

/// Room among the tabs where the dragged one would land. The tab keeps its
/// width, clipped from the right, and fades in while the gap opens; a gap
/// that shuts is empty.
#[derive(IntoElement)]
struct TabGap {
    id: ElementId,
    open: bool,
    width: Pixels,
    tab: Option<AnyElement>,
}

impl RenderOnce for TabGap {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let share = glide(self.id, self.open, BASE, window, cx);
        h_flex()
            .flex_none()
            .h_full()
            .w(self.width * share)
            .overflow_hidden()
            .opacity(share * share)
            .children(self.tab)
    }
}
