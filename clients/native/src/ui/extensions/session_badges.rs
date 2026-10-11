//! Icons of the extensions that only read the open session.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::*;
use gpui_kit::*;

use super::badge::{Details, badge_frame, icon_with_count};
use super::host::{Host, HostView};
use super::session_info::{
    AGENT, CONTEXT, PLAN, SESSION, completed, current_plan, model_label, session_stats,
};
use crate::tool_display::StepStatus;
use crate::ui::context_usage::tokens;
use crate::ui::controls::mode_label;

type Face = fn(&HostView, &App) -> (AnyElement, Details);

/// An icon whose face and tooltip come straight from host state.
struct HostBadge {
    host: Host,
    id: &'static str,
    face: Face,
}

impl Render for HostBadge {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app: &App = cx;
        let (face, details) = self
            .host
            .read(app, |host| (self.face)(host, app))
            .unwrap_or_else(|| {
                let icon = super::find(self.id).map(|manifest| (manifest.icon)().small());
                (
                    div().children(icon).into_any_element(),
                    Details::new("Нет данных"),
                )
            });
        badge_frame(self.id, face, details, &self.host, app)
    }
}

fn host_badge(host: Host, id: &'static str, face: Face, cx: &mut App) -> AnyView {
    cx.new(|_| HostBadge { host, id, face }).into()
}

/// Icon that is just the extension icon, described by its manifest.
struct PlainBadge {
    host: Host,
    id: &'static str,
}

impl Render for PlainBadge {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let manifest = super::find(self.id);
        let face = div().children(manifest.map(|manifest| (manifest.icon)().small()));
        let details = match manifest {
            Some(manifest) => Details::new(manifest.title).line(manifest.description),
            None => Details::new(self.id),
        };
        badge_frame(self.id, face, details, &self.host, cx)
    }
}

pub(super) fn plain_badge(host: Host, id: &'static str, cx: &mut App) -> AnyView {
    cx.new(|_| PlainBadge { host, id }).into()
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
        .size(px(22.))
        .child(
            div()
                .text_size(px(9.))
                .font_semibold()
                .child(format!("{percent:.0}")),
        )
}

fn context(host: &HostView, cx: &App) -> (AnyElement, Details) {
    let Some(usage) = host.session().and_then(|session| session.context) else {
        return (
            Icon::new(Lucide::Layers).small().into_any_element(),
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
        ring("context-badge-ring", percent, usage.color(cx.theme())).into_any_element(),
        details,
    )
}

fn plan(host: &HostView, _: &App) -> (AnyElement, Details) {
    let steps = current_plan(host);
    let icon = Icon::new(Lucide::ListChecks);
    if steps.is_empty() {
        return (
            icon.small().into_any_element(),
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
    (
        icon_with_count(icon, Some(format!("{done}/{}", steps.len()))).into_any_element(),
        details,
    )
}

fn session(host: &HostView, cx: &App) -> (AnyElement, Details) {
    let icon = Icon::new(IconName::Info).small();
    let Some(stats) = session_stats(host) else {
        return (
            icon.into_any_element(),
            Details::new("Сессия").line("Сессия не открыта."),
        );
    };
    let icon = if stats.attention {
        icon.text_color(cx.theme().warning)
    } else {
        icon
    };
    (
        icon.into_any_element(),
        Details::new(format!("Сессия: {}", stats.status.to_lowercase()))
            .line(format!("Сообщений: {}", stats.messages))
            .line(format!("Вызовов инструментов: {}", stats.tools))
            .line(format!("Ожидают ответа: {}", stats.waiting))
            .line(format!("В очереди: {}", stats.queued)),
    )
}

fn agent(host: &HostView, _: &App) -> (AnyElement, Details) {
    let icon = Icon::new(IconName::Bot).small().into_any_element();
    let Some(config) = host.session().and_then(|session| session.config.as_ref()) else {
        return (icon, Details::new("Агент").line("Загрузка…"));
    };
    (
        icon,
        Details::new(format!("Профиль {}", config.profile))
            .line(format!("Модель: {}", model_label(config)))
            .line(format!("Режим: {}", mode_label(&config.permission_mode)))
            .line(format!("Инструментов: {}", config.registered_tools.len())),
    )
}
