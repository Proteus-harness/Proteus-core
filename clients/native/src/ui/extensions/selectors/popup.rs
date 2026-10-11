//! The chooser a selector icon opens next to itself: permission modes, or
//! models with the reasoning effort slider under them. The chosen option is
//! marked by colour rather than a check.

use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::config::ConfigSummary;

use super::{Control, SelectorBadge};
use crate::ui::chat::Chat;
use crate::ui::chat::controls::{MODES, current_effort, efforts, mode_from_value, models};
use crate::ui::prefs;

/// Room between the icon and its chooser.
const GAP: f32 = 4.;

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
        // from the composer and the status bar.
        let below = icon.center().y < window.viewport_size().height / 2.;
        let (anchor, position) = if below {
            (Anchor::TopLeft, icon.bottom_left() + point(px(0.), px(GAP)))
        } else {
            (Anchor::BottomLeft, icon.origin - point(px(0.), px(GAP)))
        };
        let content = match self.control {
            Control::Mode => mode_choices(config, chat, cx),
            Control::Model => self.model_choices(config, chat, cx),
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
                        .occlude()
                        .popover_style(cx)
                        .p_1()
                        .min_w(prefs::scaled(240.))
                        .max_w(prefs::scaled(360.))
                        .text_sm()
                        // A press on the icon toggles it itself.
                        .on_mouse_down_out(move |event, _, cx| {
                            if !trigger.get().contains(&event.position) {
                                let _ = this.update(cx, |this, cx| this.set_open(false, cx));
                            }
                        })
                        .children(content),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }

    fn model_choices(
        &self,
        config: &ConfigSummary,
        chat: WeakEntity<Chat>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let current = config.model.as_ref().map(|model| model.name.clone());
        let mut content = Vec::new();
        let models = models(config);
        if !models.is_empty() {
            content.push(heading("Модель", cx).into_any_element());
            let rows = models
                .into_iter()
                .enumerate()
                .map(|(index, (name, label))| {
                    let selected = current.as_ref() == Some(&name);
                    let chat = chat.clone();
                    let this = cx.entity().downgrade();
                    choice(("model", index), label, selected, cx, move |cx| {
                        let _ = chat.update(cx, |chat, cx| chat.set_model(name.clone(), cx));
                        let _ = this.update(cx, |this, cx| this.set_open(false, cx));
                    })
                });
            content.push(
                v_flex()
                    .id("selector-models")
                    .max_h(prefs::scaled(320.))
                    .overflow_y_scroll()
                    .children(rows)
                    .into_any_element(),
            );
        }
        if let Some(slider) = &self.effort {
            if !content.is_empty() {
                content.push(divider(cx).into_any_element());
            }
            content.push(effort_section(config, &slider.state, chat, cx).into_any_element());
        }
        content
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
            choice(("mode", index), *label, value == mode, cx, move |cx| {
                let _ = chat.update(cx, |chat, cx| chat.set_mode(value, cx));
                let _ = this.update(cx, |this, cx| this.set_open(false, cx));
            })
            .into_any_element()
        })
        .collect()
}

/// Effort slider with the effort names under its notches; a name can be
/// clicked too, and «по профилю» drops the session's override.
fn effort_section(
    config: &ConfigSummary,
    state: &Entity<SliderState>,
    chat: WeakEntity<Chat>,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let (muted, primary) = (theme.muted_foreground, theme.primary);
    let current = current_effort(config);
    let reset = {
        let chat = chat.clone();
        let inherited = current.is_none();
        div()
            .id("effort-profile")
            .text_xs()
            .cursor_pointer()
            .text_color(if inherited { primary } else { muted })
            .hover(move |style| style.text_color(primary))
            .child("по профилю")
            .on_click(move |_, _, cx| {
                let _ = chat.update(cx, |chat, cx| chat.set_effort(None, cx));
            })
    };
    let names = efforts(config).iter().map(|effort| {
        let selected = current.as_ref() == Some(effort);
        let chat = chat.clone();
        let value = effort.clone();
        div()
            .id(SharedString::from(format!("effort-{effort}")))
            .text_xs()
            .cursor_pointer()
            .text_color(if selected { primary } else { muted })
            .hover(move |style| style.text_color(primary))
            .child(effort.clone())
            .on_click(move |_, _, cx| {
                let value = value.clone();
                let _ = chat.update(cx, |chat, cx| chat.set_effort(Some(value), cx));
            })
    });
    v_flex()
        .px_2()
        .pt_1()
        .pb_1p5()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .child(div().text_xs().text_color(muted).child("Рассуждения"))
                .child(reset),
        )
        .child(Slider::new(state).horizontal().py_1())
        .child(h_flex().justify_between().gap_2().children(names))
}

fn heading(text: &'static str, cx: &App) -> Div {
    div()
        .px_2()
        .pt_1()
        .pb_0p5()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

fn divider(cx: &App) -> Div {
    div().my_1().h_px().bg(cx.theme().border)
}

/// A row of the chooser; the chosen one is tinted with the accent colour.
fn choice(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    cx: &App,
    on_choose: impl Fn(&mut App) + 'static,
) -> Stateful<Div> {
    let theme = cx.theme();
    let (hover, primary) = (theme.list_hover, theme.primary);
    div()
        .id(id.into())
        .px_2()
        .py_1()
        .rounded(theme.radius)
        .cursor_pointer()
        .truncate()
        .when(selected, |row| {
            row.text_color(primary)
                .font_medium()
                .bg(primary.opacity(0.12))
        })
        .when(!selected, |row| row.hover(move |style| style.bg(hover)))
        .child(label.into())
        .on_click(move |_, _, cx| on_choose(cx))
}
