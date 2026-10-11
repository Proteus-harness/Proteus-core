//! Icons of the extensions that only read the open session.

use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::*;
use gpui_kit::*;

use super::badge::{Details, Face, badge_frame, icon_with_count};
use super::host::{Host, HostView};
use super::session_info::{
    AGENT, CONTEXT, PLAN, SESSION, completed, current_plan, model_label, session_stats,
};
use crate::tool_display::StepStatus;
use crate::ui::chat::context_usage::tokens;
use crate::ui::chat::controls::mode_label;
use crate::ui::motion::Number;
use crate::ui::prefs;

type Read = fn(&HostView, &App) -> (Face, Details);

/// An icon whose face and tooltip come straight from host state.
struct HostBadge {
    host: Host,
    id: &'static str,
    read: Read,
}

impl Render for HostBadge {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app: &App = cx;
        let id = self.id;
        let (face, details) = self
            .host
            .read(app, |host| (self.read)(host, app))
            .unwrap_or_else(|| (icon_face(id), Details::new("Нет данных")));
        badge_frame(id, face, details, &self.host, app)
    }
}

fn host_badge(host: Host, id: &'static str, read: Read, cx: &mut App) -> AnyView {
    cx.new(|_| HostBadge { host, id, read }).into()
}

/// Face that is just the manifest icon.
fn icon_face(id: &'static str) -> Face {
    Rc::new(move || {
        div()
            .children(super::find(id).map(|manifest| (manifest.icon)().small()))
            .into_any_element()
    })
}

pub(super) fn build_context(host: Host, _: &mut Window, cx: &mut App) -> AnyView {
    host_badge(host, CONTEXT, context, cx)
}

pub(super) fn build_plan(host: Host, _: &mut Window, cx: &mut App) -> AnyView {
    host_badge(host, PLAN, plan, cx)
}

pub(super) fn build_session(host: Host, _: &mut Window, cx: &mut App) -> AnyView {
    host_badge(host, SESSION, session, cx)
}

pub(super) fn build_agent(host: Host, _: &mut Window, cx: &mut App) -> AnyView {
    host_badge(host, AGENT, agent, cx)
}

/// A ring of the used share with the percent inside.
pub(super) fn ring(id: &'static str, percent: f32, color: Hsla) -> impl IntoElement {
    ProgressCircle::new(id)
        .value(percent)
        .color(color)
        .size(prefs::scaled(22.))
        .child(div().text_size(px(9.)).font_semibold().child(Number::new(
            ElementId::Name(format!("{id}-number").into()),
            percent,
            |value| format!("{value:.0}"),
        )))
}

pub(super) fn ring_face(id: &'static str, percent: f32, color: Hsla) -> Face {
    Rc::new(move || ring(id, percent, color).into_any_element())
}

fn context(host: &HostView, cx: &App) -> (Face, Details) {
    let Some(usage) = host.session().and_then(|session| session.context) else {
        return (
            icon_face(CONTEXT),
            Details::new("Контекст").line("Появится после первого запроса к модели."),
        );
    };
    let percent = usage.percent();
    let mut details = Details::new(format!("Контекст: {percent:.0}%")).line(format!(
        "Занято {} из {}",
        tokens(usage.used),
        tokens(usage.max)
    ));
    if let Some(trigger) = usage.trigger {
        details = details.line(format!("Автокомпакт при {}", tokens(trigger)));
    }
    (
        ring_face("context-badge-ring", percent, usage.color(cx.theme())),
        details,
    )
}

fn plan(host: &HostView, _: &App) -> (Face, Details) {
    let steps = current_plan(host);
    if steps.is_empty() {
        return (
            icon_face(PLAN),
            Details::new("План").line("Агент ещё не составил план."),
        );
    }
    let done = completed(&steps);
    let mut details = Details::new(format!("План: {done} из {}", steps.len()));
    for step in &steps {
        let mark = match step.status {
            StepStatus::Completed => "✓",
            StepStatus::InProgress => "•",
            StepStatus::Pending => "○",
        };
        details = details.line(format!("{mark} {}", step.step));
    }
    let count = format!("{done}/{}", steps.len());
    let face: Face = Rc::new(move || {
        icon_with_count(Icon::new(Lucide::ListChecks), Some(count.clone())).into_any_element()
    });
    (face, details)
}

fn session(host: &HostView, cx: &App) -> (Face, Details) {
    let Some(stats) = session_stats(host) else {
        return (
            icon_face(SESSION),
            Details::new("Сессия").line("Сессия не открыта."),
        );
    };
    let color = stats.attention.then_some(cx.theme().warning);
    let face: Face = Rc::new(move || {
        let icon = Icon::new(IconName::Info).small();
        match color {
            Some(color) => icon.text_color(color).into_any_element(),
            None => icon.into_any_element(),
        }
    });
    (
        face,
        Details::new(format!("Сессия: {}", stats.status.to_lowercase()))
            .line(format!("Сообщений: {}", stats.messages))
            .line(format!("Вызовов инструментов: {}", stats.tools))
            .line(format!("Ожидают ответа: {}", stats.waiting))
            .line(format!("В очереди: {}", stats.queued)),
    )
}

fn agent(host: &HostView, _: &App) -> (Face, Details) {
    let Some(config) = host.session().and_then(|session| session.config.as_ref()) else {
        return (icon_face(AGENT), Details::new("Агент").line("Загрузка…"));
    };
    let mut details = Details::new(format!("Профиль {}", config.profile))
        .line(format!("Модель: {}", model_label(config)))
        .line(format!("Режим: {}", mode_label(&config.permission_mode)))
        .line(format!("Инструментов: {}", config.registered_tools.len()));
    for module in &config.modules {
        details = details.line(format!("{}: {}", module.slot, module.id));
    }
    (icon_face(AGENT), details)
}
