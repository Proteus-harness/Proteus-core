//! Placement and focus of any process extension's badge popup.

use super::SurfaceKind;
use super::badge::DeclarativeBadge;
use crate::ui::prefs;
use gpui_kit::component::*;
use gpui_kit::*;

impl DeclarativeBadge {
    pub(super) fn toggle_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_some() {
            self.close_popup(window, cx);
        } else {
            self.restore = window.focused(cx);
            let popup = super::surface(self.host.clone(), SurfaceKind::Popup, None, window, cx);
            popup.focus.focus(window, cx);
            self.popup = Some(popup);
        }
        cx.notify();
    }

    pub(super) fn close_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let popup = self.popup.take();
        if popup
            .as_ref()
            .is_some_and(|popup| popup.focus.contains_focused(window, cx))
            && let Some(restore) = self.restore.take()
        {
            restore.focus(window, cx);
        }
        cx.notify();
    }

    pub(super) fn render_popup(
        &self,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let popup = self.popup.as_ref()?;
        let bounds = self.bounds.get();
        let below = bounds.center().y < window.viewport_size().height / 2.;
        let (anchor, position) = if below {
            (
                Anchor::TopLeft,
                bounds.bottom_left() + point(px(0.), px(4.)),
            )
        } else {
            (Anchor::BottomLeft, bounds.origin - point(px(0.), px(4.)))
        };
        let this = cx.entity().downgrade();
        let trigger = self.bounds.clone();
        Some(
            deferred(
                anchored()
                    .anchor(anchor)
                    .position(position)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        v_flex()
                            .id("extension-popup")
                            .track_focus(&popup.focus)
                            .occlude()
                            .popover_style(cx)
                            .w(prefs::scaled(width))
                            .max_h(prefs::scaled(420.))
                            .text_sm()
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                                if event.keystroke.key == "tab" {
                                    if let Some(popup) = &this.popup {
                                        crate::ui::kit::cycle_focus(
                                            &popup.focus,
                                            !event.keystroke.modifiers.shift,
                                            window,
                                            cx,
                                        );
                                    }
                                    cx.stop_propagation();
                                } else if event.keystroke.key == "escape" {
                                    let action = super::installed(this.host.extension())
                                        .and_then(|extension| {
                                            extension.manifest.surfaces.popup.as_ref()
                                        })
                                        .and_then(|popup| popup.escape_action.clone());
                                    if let Some(action) = action {
                                        this.view.update(cx, |view, cx| view.act(action, None, cx));
                                    } else {
                                        this.close_popup(window, cx);
                                    }
                                    cx.stop_propagation();
                                }
                            }))
                            .on_mouse_down_out(move |event, window, cx| {
                                if !trigger.get().contains(&event.position) {
                                    let this = this.clone();
                                    window.defer(cx, move |window, cx| {
                                        let _ = this
                                            .update(cx, |this, cx| this.close_popup(window, cx));
                                    });
                                }
                            })
                            .child(popup.view.clone())
                            .focus_trap("extension-popup-trap", &popup.focus),
                    ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}
