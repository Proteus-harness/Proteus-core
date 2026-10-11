use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::extensions::{self, DraggedWidget, Host, HostEvent};
use super::motion::reveal_width;
use super::prefs;
use super::workspace::Workspace;

pub(super) const PANEL_WIDTH: f32 = 300.;
const CLOSE_MOTION: Duration = Duration::from_millis(320);

/// Right side panel: a column of extension widgets in the order the user
/// arranged them. Widgets are built on first show and kept while enabled.
#[derive(Default)]
pub(super) struct InfoPanel {
    pub open: bool,
    closing: Option<Task<()>>,
    widgets: HashMap<&'static str, AnyView>,
}

impl Workspace {
    pub(super) fn toggle_panel(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.panel.open = !self.panel.open;
        if self.panel.open {
            self.panel.closing = None;
            cx.emit(HostEvent::PanelOpened);
        } else {
            self.panel.closing = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(CLOSE_MOTION).await;
                let _ = this.update(cx, |this, cx| {
                    this.panel.closing = None;
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }

    pub(super) fn render_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if (!self.panel.open && self.panel.closing.is_none()) || self.settings.is_some() {
            return None;
        }
        let visible = extensions::visible_panels(cx);
        self.panel
            .widgets
            .retain(|id, _| visible.iter().any(|manifest| manifest.id == *id));
        let host = Host::new(cx.weak_entity());
        let mut slots = Vec::new();
        for (index, manifest) in visible.iter().enumerate() {
            let Some(surface) = &manifest.panel else {
                continue;
            };
            let view = self
                .panel
                .widgets
                .entry(manifest.id)
                .or_insert_with(|| (surface.build)(host.clone(), window, cx))
                .clone();
            slots.push(drop_slot(("widget-slot", index), Some(manifest.id), cx).child(view));
        }
        let theme = cx.theme();
        let content = v_flex()
            .id("info-panel")
            .w(px(PANEL_WIDTH))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .children(slots)
            .child(
                drop_slot(("widget-slot", usize::MAX), None, cx)
                    .flex_1()
                    .min_h(px(48.)),
            )
            .when(visible.is_empty(), |panel| {
                panel.child(
                    div()
                        .p_4()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(
                            "Все виджеты отключены. Включите их в настройках, раздел «Расширения».",
                        ),
                )
            });
        Some(reveal_width(
            h_flex()
                .flex_none()
                .h_full()
                .justify_end()
                .overflow_hidden()
                .child(content),
            "info-panel-reveal",
            self.panel.open,
            px(PANEL_WIDTH),
        ))
    }
}

/// Drop target placing a dragged widget before `before` (or last).
fn drop_slot(id: impl Into<ElementId>, before: Option<&'static str>, cx: &App) -> Stateful<Div> {
    let accent = cx.theme().primary;
    div()
        .id(id)
        .border_t_2()
        .border_color(transparent_black())
        .drag_over::<DraggedWidget>(move |style, _, _, _| style.border_color(accent))
        .on_drop(move |dragged: &DraggedWidget, _, cx| {
            let known = extensions::panel_ids();
            prefs::update(cx, |preferences| {
                preferences
                    .extensions
                    .move_before(&known, dragged.id, before)
            });
        })
}
