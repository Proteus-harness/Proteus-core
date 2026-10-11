//! Icons that change the active session: permission mode, model and
//! reasoning effort. They are ordinary icons, so they can be moved out of
//! the composer like any other.

use gpui_kit::*;

use super::badge::{Details, Face, badge_frame};
use super::host::Host;
use crate::ui::chat::controls::{self, Control, effort_of, mode_label};

pub(super) const MODE: &str = "mode";
pub(super) const MODEL: &str = "model";
pub(super) const EFFORT: &str = "effort";

struct SelectorBadge {
    host: Host,
    id: &'static str,
    control: Control,
}

impl Render for SelectorBadge {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app: &App = cx;
        let Some(chat) = self.host.chat(app) else {
            return div().into_any_element();
        };
        let Some(config) = chat.read(app).session.config.clone() else {
            return div().into_any_element();
        };
        let chat = chat.downgrade();
        let control = self.control;
        // Nothing to choose from: the icon stays out of the way.
        if controls::selector(control, &config, chat.clone()).is_none() {
            return div().into_any_element();
        }
        let details = match control {
            Control::Mode => Details::new("Режим разрешений")
                .line(format!("Сейчас: {}", mode_label(&config.permission_mode)))
                .line("Что агент делает без подтверждения."),
            Control::Model => Details::new("Модель").line(format!("Профиль {}", config.profile)),
            Control::Effort => Details::new("Рассуждения")
                .line(format!("Сейчас: {}", effort_of(&config)))
                .line("Сколько модель думает перед ответом."),
        };
        let config = std::rc::Rc::new(config);
        let face: Face = std::rc::Rc::new(move || {
            controls::selector(control, &config, chat.clone())
                .unwrap_or_else(|| div().into_any_element())
        });
        badge_frame(self.id, face, details, &self.host, app).into_any_element()
    }
}

fn build(host: Host, id: &'static str, control: Control, cx: &mut App) -> AnyView {
    cx.new(|_| SelectorBadge { host, id, control }).into()
}

pub(super) fn build_mode(host: Host, _: &mut Window, cx: &mut App) -> AnyView {
    build(host, MODE, Control::Mode, cx)
}

pub(super) fn build_model(host: Host, _: &mut Window, cx: &mut App) -> AnyView {
    build(host, MODEL, Control::Model, cx)
}

pub(super) fn build_effort(host: Host, _: &mut Window, cx: &mut App) -> AnyView {
    build(host, EFFORT, Control::Effort, cx)
}
