//! Icons that change the active session: its permission mode, and its model
//! together with the reasoning effort. They are ordinary icons, so they can
//! be dragged out of the composer like any other; a click opens their
//! chooser next to the icon.

mod popup;

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::ElementExt as _;
use gpui_kit::component::slider::{SliderEvent, SliderState};
use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::app_protocol::config::ConfigSummary;

use super::badge::{Details, Face, Press, pressable_badge};
use super::host::Host;
use crate::ui::chat::Chat;
use crate::ui::chat::controls::{
    current_effort, default_effort, effort_of, efforts, mode_label, model_label, models,
};

pub(super) const MODE: &str = "mode";
pub(super) const MODEL: &str = "model";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Mode,
    Model,
}

/// Slider over the efforts of the current model; rebuilt when their number
/// changes.
struct EffortSlider {
    state: Entity<SliderState>,
    count: usize,
    /// Position last taken from the session, so a drag isn't undone before
    /// the server confirms it.
    synced: Option<usize>,
    _changes: Subscription,
}

struct SelectorBadge {
    host: Host,
    id: &'static str,
    control: Control,
    open: bool,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    effort: Option<EffortSlider>,
}

impl SelectorBadge {
    fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.open != open {
            self.open = open;
            cx.notify();
        }
    }

    fn session(&self, cx: &App) -> Option<(Entity<Chat>, ConfigSummary)> {
        let chat = self.host.chat(cx)?;
        let config = chat.read(cx).session.config.clone()?;
        Some((chat, config))
    }

    /// Keeps the effort slider in step with the session's efforts.
    fn sync_effort(&mut self, config: &ConfigSummary, window: &mut Window, cx: &mut Context<Self>) {
        let count = efforts(config).len();
        if count < 2 {
            self.effort = None;
            return;
        }
        if self
            .effort
            .as_ref()
            .is_none_or(|slider| slider.count != count)
        {
            let state = cx.new(|_| SliderState::new().min(0.).max((count - 1) as f32).step(1.));
            let changes = cx.subscribe(&state, |this, _, event: &SliderEvent, cx| {
                if let SliderEvent::Release(value) = event {
                    this.choose_effort(value.start().round() as usize, cx);
                }
            });
            self.effort = Some(EffortSlider {
                state,
                count,
                synced: None,
                _changes: changes,
            });
        }
        let position = effort_position(config);
        if let Some(slider) = &mut self.effort
            && slider.synced != Some(position)
        {
            slider.synced = Some(position);
            slider
                .state
                .update(cx, |state, cx| state.set_value(position as f32, window, cx));
        }
    }

    fn choose_effort(&mut self, position: usize, cx: &mut Context<Self>) {
        let Some((chat, config)) = self.session(cx) else {
            return;
        };
        let Some(effort) = efforts(&config).get(position).cloned() else {
            return;
        };
        if current_effort(&config).as_ref() != Some(&effort) {
            chat.update(cx, |chat, cx| chat.set_effort(Some(effort), cx));
        }
    }
}

/// Slider position of the session's effort; the profile default when there
/// is no override.
fn effort_position(config: &ConfigSummary) -> usize {
    let current = current_effort(config);
    let wanted = current.as_deref().or_else(|| default_effort(config));
    wanted
        .and_then(|wanted| efforts(config).iter().position(|effort| effort == wanted))
        .unwrap_or(0)
}

impl Render for SelectorBadge {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((chat, config)) = self.session(cx) else {
            self.open = false;
            return div().into_any_element();
        };
        if self.control == Control::Model {
            // Nothing to choose from: the icon stays out of the way.
            if models(&config).is_empty() && efforts(&config).len() < 2 {
                self.open = false;
                return div().into_any_element();
            }
            self.sync_effort(&config, window, cx);
        }
        let (label, details) = match self.control {
            Control::Mode => (
                mode_label(&config.permission_mode).to_owned(),
                Details::new("Режим разрешений")
                    .line(format!("Сейчас: {}", mode_label(&config.permission_mode)))
                    .line("Что агент делает без подтверждения."),
            ),
            Control::Model => (
                model_face(&config),
                Details::new("Модель и рассуждения")
                    .line(format!("Профиль {}", config.profile))
                    .line(format!(
                        "Сейчас: {}, {}",
                        model_label(&config),
                        effort_of(&config)
                    )),
            ),
        };
        let open = self.open;
        let label = SharedString::from(label);
        let face: Face = Rc::new(move || {
            h_flex()
                .gap_0p5()
                .child(label.clone())
                .child(
                    Icon::new(if open {
                        Lucide::ChevronUp
                    } else {
                        Lucide::ChevronDown
                    })
                    .xsmall(),
                )
                .into_any_element()
        });
        let this = cx.entity().downgrade();
        let press: Press = Rc::new(move |_, cx| {
            let _ = this.update(cx, |this, cx| this.set_open(!this.open, cx));
        });
        let bounds = self.bounds.clone();
        let popup = self
            .open
            .then(|| self.render_popup(&config, chat.downgrade(), window, cx));
        div()
            .relative()
            .child(pressable_badge(
                self.id,
                face,
                details,
                &self.host,
                Some(press),
                cx,
            ))
            .on_prepaint(move |rect, _, _| bounds.set(rect))
            .children(popup)
            .into_any_element()
    }
}

/// Model name with the effort after it, as short as the bar allows.
fn model_face(config: &ConfigSummary) -> String {
    let model = model_label(config);
    match current_effort(config) {
        Some(effort) if efforts(config).len() > 1 => format!("{model} · {effort}"),
        _ => model,
    }
}

fn build(host: Host, id: &'static str, control: Control, cx: &mut App) -> AnyView {
    cx.new(|_| SelectorBadge {
        host,
        id,
        control,
        open: false,
        bounds: Rc::default(),
        effort: None,
    })
    .into()
}

pub(super) fn build_mode(host: Host, _: &mut Window, cx: &mut App) -> AnyView {
    build(host, MODE, Control::Mode, cx)
}

pub(super) fn build_model(host: Host, _: &mut Window, cx: &mut App) -> AnyView {
    build(host, MODEL, Control::Model, cx)
}
