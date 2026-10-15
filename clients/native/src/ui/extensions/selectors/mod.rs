//! Icons that change the active session: its permission mode, and its model
//! together with the reasoning effort. They are ordinary icons, so they can
//! be dragged out of the composer like any other; a click opens their
//! chooser next to the icon.

mod model;
mod popup;
mod steps;

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::ElementExt as _;
use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::app_protocol::config::ConfigSummary;

use super::badge::{Details, Face, Press, pressable_badge};
use super::host::Host;
use crate::ui::chat::Chat;
use crate::ui::chat::controls::{
    current_effort, default_effort, efforts, mode_label, model_label, models,
};

pub(super) const MODE: &str = "mode";
pub(super) const MODEL: &str = "model";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Mode,
    Model,
}

struct SelectorBadge {
    host: Host,
    id: &'static str,
    control: Control,
    open: bool,
    /// The model chooser shows the model list instead of the effort.
    models_open: bool,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Effort step under the pointer while it is dragged along the steps.
    scrub: Option<usize>,
    /// Step already sent to the server, shown until the session reports it.
    pending: Option<usize>,
    /// Where the effort pill was laid out, for pointer positions.
    track: Rc<Cell<Bounds<Pixels>>>,
    /// Step the effort thumb is drawn at while it slides.
    thumb: Rc<Cell<Option<f32>>>,
    /// Holds the keys while the chooser is open, so Esc reaches it.
    focus: FocusHandle,
    /// What had the keys before the chooser took them.
    restore: Option<FocusHandle>,
}

impl SelectorBadge {
    fn open_chooser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restore = window.focused(cx);
        self.set_open(true, cx);
        self.focus.focus(window, cx);
    }

    /// Closes the chooser and hands the keys back, unless something else
    /// has taken them meanwhile.
    fn close_chooser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_open(false, cx);
        let restore = self.restore.take();
        if self.focus.contains_focused(window, cx)
            && let Some(restore) = restore
        {
            restore.focus(window, cx);
        }
    }

    /// Esc steps back from the model list first, then closes.
    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.models_open {
            self.show_models(false, cx);
        } else {
            self.close_chooser(window, cx);
        }
    }

    fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.open != open {
            self.open = open;
            self.models_open = false;
            self.scrub = None;
            self.pending = None;
            self.thumb.set(None);
            cx.notify();
        }
    }

    fn show_models(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.models_open != shown {
            self.models_open = shown;
            cx.notify();
        }
    }

    fn session(&self, cx: &App) -> Option<(Entity<Chat>, ConfigSummary)> {
        let chat = self.host.chat(cx)?;
        let config = chat.read(cx).session.config.clone()?;
        Some((chat, config))
    }

    fn scrub_to(&mut self, position: usize, cx: &mut Context<Self>) {
        if self.scrub != Some(position) {
            self.scrub = Some(position);
            cx.notify();
        }
    }

    /// Moves the effort a step from the shown one, as a click on the track.
    fn step_effort(&mut self, offset: isize, cx: &mut Context<Self>) {
        let Some((_, config)) = self.session(cx) else {
            return;
        };
        let count = efforts(&config).len();
        if self.models_open || count < 2 {
            return;
        }
        let shown = self
            .pending
            .unwrap_or_else(|| effort_position(&config))
            .min(count - 1);
        let next = shown.saturating_add_signed(offset).min(count - 1);
        if next != shown {
            self.scrub = Some(next);
            self.finish_scrub(cx);
        }
    }

    /// Sends the step the drag ended on.
    fn finish_scrub(&mut self, cx: &mut Context<Self>) {
        let Some(position) = self.scrub.take() else {
            return;
        };
        cx.notify();
        let Some((chat, config)) = self.session(cx) else {
            return;
        };
        let Some(effort) = efforts(&config).get(position).cloned() else {
            return;
        };
        if current_effort(&config).as_ref() != Some(&effort) {
            self.pending = Some(position);
            chat.update(cx, |chat, cx| chat.set_effort(Some(effort), cx));
        }
    }
}

/// Step of the session's effort; the profile default when there is no
/// override.
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
            if self.pending == Some(effort_position(&config)) {
                self.pending = None;
            }
        }
        let (label, details) = match self.control {
            Control::Mode => (
                mode_label(&config.permission_mode).to_owned(),
                Details::new("Режим разрешений").line("Что агент делает без подтверждения."),
            ),
            Control::Model => (
                model_face(&config),
                Details::new("Модель и рассуждения").line(match &config.model {
                    Some(model) => format!("Профиль {} · {}", config.profile, model.provider),
                    None => format!("Профиль {}", config.profile),
                }),
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
        let press: Press = Rc::new(move |window, cx| {
            let _ = this.update(cx, |this, cx| {
                if this.open {
                    this.close_chooser(window, cx);
                } else {
                    this.open_chooser(window, cx);
                }
            });
        });
        let bounds = self.bounds.clone();
        let popup = self
            .open
            .then(|| self.render_popup(&config, chat.downgrade(), window, cx));
        // The measuring layer goes first: an absolute child without offsets
        // sits after the siblings before it.
        div()
            .relative()
            .on_prepaint(move |rect, _, _| bounds.set(rect))
            .child(pressable_badge(
                self.id,
                face,
                details,
                &self.host,
                Some(press),
                cx,
            ))
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
    cx.new(|cx| SelectorBadge {
        host,
        id,
        control,
        open: false,
        models_open: false,
        bounds: Rc::default(),
        scrub: None,
        pending: None,
        track: Rc::default(),
        thumb: Rc::default(),
        focus: cx.focus_handle(),
        restore: None,
    })
    .into()
}

pub(super) fn build_mode(host: Host, _: &mut Window, cx: &mut App) -> AnyView {
    build(host, MODE, Control::Mode, cx)
}

pub(super) fn build_model(host: Host, _: &mut Window, cx: &mut App) -> AnyView {
    build(host, MODEL, Control::Model, cx)
}
