//! The side panel between the rail and the tabs: one panel at a time,
//! opened from the rail, as wide as its edge is dragged; a double click on
//! the edge gives back the default width. It slides open and closed, and a
//! panel replacing another fades in.

use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::*;

use crate::preferences::{SIDEBAR_WIDTH, SidebarPreferences};
use crate::tabs::Axis;
use crate::ui::workspace::Workspace;
use crate::ui::{drag, motion, prefs, tabs};

/// Payload of the side panel's edge while it is dragged.
#[derive(Clone)]
pub(in crate::ui) struct SidebarDrag;

impl Render for SidebarDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Workspace {
    pub(in crate::ui) fn render_sidebar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let shown = self.shown_panel(cx);
        let open = motion::glide("sidebar".into(), shown.is_some(), motion::BASE, window, cx);
        // A closed panel keeps showing while it slides away.
        let panel = match shown {
            Some(panel) => {
                self.sidebar_last = Some(panel.name);
                panel
            }
            None if open > 0. => super::info(self.sidebar_last?, cx)?,
            None => return None,
        };
        // A drag released outside the window drops the width it reached.
        if !cx.has_active_drag() {
            self.sidebar_drag = None;
        }
        let width = prefs::scaled((self.sidebar_drag).unwrap_or(prefs::current(cx).sidebar.width));
        let content = motion::fade_in_if(
            div()
                .size_full()
                .child(self.render_panel(&panel, window, cx)),
            ElementId::Name(format!("sidebar-panel-{}", panel.name).into()),
            true,
        );
        let name = SharedString::from("sidebar-edge");
        let grip = tabs::divider_grip(name.clone(), Axis::Row, cx)
            .on_drag(SidebarDrag, move |drag, _, _, cx| {
                drag::start(name.clone(), cx);
                cx.new(|_| drag.clone())
            })
            .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    this.sidebar_drag = None;
                    let width = SidebarPreferences::default().width;
                    prefs::update(cx, |prefs| prefs.sidebar.width = width);
                }
            }));
        // The background stays outside the fade, so a switched panel does
        // not flash the window color through.
        let theme = cx.theme();
        let side = div()
            .flex()
            .flex_none()
            .h_full()
            .w(width)
            .on_drag_move::<SidebarDrag>(cx.listener(
                |this, event: &DragMoveEvent<SidebarDrag>, _, cx| {
                    let offset = event.event.position.x - event.bounds.left();
                    let width = offset.as_f32() / prefs::scale(cx);
                    this.sidebar_drag =
                        Some(width.clamp(*SIDEBAR_WIDTH.start(), *SIDEBAR_WIDTH.end()));
                    cx.notify();
                },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .bg(theme.sidebar)
                    .text_color(theme.sidebar_foreground)
                    .child(content),
            )
            .child(tabs::divider_line(Axis::Row, grip, cx));
        // The panel keeps its width and slides out from under the rail, so
        // its content does not reflow while it opens or closes.
        let slot = h_flex()
            .flex_none()
            .h_full()
            .justify_end()
            .overflow_hidden()
            .w(width * open)
            .child(side);
        Some(slot.into_any_element())
    }

    /// Stores the width the side panel's edge was released at.
    pub(in crate::ui) fn finish_sidebar_resize(&mut self, cx: &mut Context<Self>) {
        if let Some(width) = self.sidebar_drag.take() {
            prefs::update(cx, |prefs| prefs.sidebar.width = width);
        }
    }
}
