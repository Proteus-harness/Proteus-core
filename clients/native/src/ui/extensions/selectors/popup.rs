//! The chooser a selector icon opens next to itself: permission modes, or
//! the model with its reasoning effort. The chosen option is marked by colour
//! rather than a check.

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::config::ConfigSummary;

use super::{Control, SelectorBadge};
use crate::ui::chat::Chat;
use crate::ui::chat::controls::{MODES, mode_from_value};
use crate::ui::fade::faded;
use crate::ui::kit::{self, Pressable as _};
use crate::ui::prefs;

/// Room between the icon and its chooser.
const GAP: f32 = 4.;
const MODEL_WIDTH: f32 = 280.;

impl SelectorBadge {
    pub(super) fn render_popup(
        &self,
        config: &ConfigSummary,
        chat: WeakEntity<Chat>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let icon = self.bounds.get();
        // Opens away from the nearer window edge: down from the top bar, up
        // from the composer.
        let below = icon.center().y < window.viewport_size().height / 2.;
        let (anchor, position) = if below {
            (Anchor::TopLeft, icon.bottom_left() + point(px(0.), px(GAP)))
        } else {
            (Anchor::BottomLeft, icon.origin - point(px(0.), px(GAP)))
        };
        let content = match self.control {
            Control::Mode => mode_choices(config, chat, cx),
            Control::Model => self.model_chooser(config, chat, cx),
        };
        let this = cx.entity().downgrade();
        let trigger = self.bounds.clone();
        deferred(
            anchored()
                .anchor(anchor)
                .position(position)
                .snap_to_window_with_margin(px(8.))
                .child(
                    v_flex()
                        .id("selector-popup")
                        .track_focus(&self.focus)
                        .occlude()
                        .popover_style(cx)
                        .p_1()
                        // The model chooser keeps one width so the effort
                        // pill does not change with the model names.
                        .map(|popup| match self.control {
                            Control::Mode => popup.min_w(prefs::scaled(240.)),
                            Control::Model => popup.w(prefs::scaled(MODEL_WIDTH)),
                        })
                        .text_sm()
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                            if !event.keystroke.modifiers.modified() {
                                this.press_key(&event.keystroke.key, window, cx);
                            }
                        }))
                        // A press on the icon toggles it itself. The press
                        // elsewhere settles focus first: the keys go back
                        // only if it left them with the chooser.
                        .on_mouse_down_out(move |event, window, cx| {
                            if !trigger.get().contains(&event.position) {
                                let this = this.clone();
                                window.defer(cx, move |window, cx| {
                                    let _ =
                                        this.update(cx, |this, cx| this.close_chooser(window, cx));
                                });
                            }
                        })
                        .children(content)
                        .focus_trap("selector-popup-trap", &self.focus),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }

    /// Esc steps back or closes, Up and Down walk the rows round, and Left
    /// and Right move the reasoning effort a step.
    fn press_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        match key {
            "escape" => self.escape(window, cx),
            "down" | "up" => kit::cycle_focus(&self.focus, key == "down", window, cx),
            "left" | "right" if matches!(self.control, Control::Model) => {
                self.step_effort(if key == "right" { 1 } else { -1 }, cx)
            }
            _ => return,
        }
        cx.stop_propagation();
    }
}

fn mode_choices(
    config: &ConfigSummary,
    chat: WeakEntity<Chat>,
    cx: &mut Context<SelectorBadge>,
) -> Vec<AnyElement> {
    let mode = mode_from_value(&config.permission_mode);
    MODES
        .iter()
        .enumerate()
        .map(|(index, (value, label))| {
            let (chat, value) = (chat.clone(), *value);
            let this = cx.entity().downgrade();
            choice(
                ("mode", index),
                *label,
                value == mode,
                cx,
                move |window, cx| {
                    let _ = chat.update(cx, |chat, cx| chat.set_mode(value, cx));
                    let _ = this.update(cx, |this, cx| this.close_chooser(window, cx));
                },
            )
            .into_any_element()
        })
        .collect()
}

pub(super) fn heading(text: SharedString, cx: &App) -> Div {
    div()
        .px_2()
        .pt_1()
        .pb_0p5()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

/// A row of the chooser; the chosen one is tinted with the accent colour.
pub(super) fn choice(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    cx: &App,
    on_choose: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let theme = cx.theme();
    let (hover, primary) = (theme.list_hover, theme.primary);
    div()
        .id(id.into())
        .px_2()
        .py_1()
        .rounded(theme.radius)
        .cursor_pointer()
        .pressable(cx)
        .when(selected, |row| {
            row.text_color(primary)
                .font_medium()
                .bg(primary.opacity(0.12))
        })
        .when(!selected, |row| row.hover(move |style| style.bg(hover)))
        .child(faded(label.into()))
        .on_click(move |_, window, cx| on_choose(window, cx))
}
