//! Widgets that only read the open session: context, plan, session and agent.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppSessionActivityStatus;
use proteus_contracts::app_protocol::config::ConfigSummary;

use super::frame::{is_open, muted, row, widget_frame};
use super::host::{Host, HostView};
use crate::tool_display::{self, PlanStep, StepStatus};
use crate::transcript::Role;
use crate::ui::context_usage::tokens;
use crate::ui::controls::mode_label;
use crate::ui::tool_row::{plan_list, tool_args};

type Content = fn(&HostView, &App) -> (Option<SharedString>, AnyElement);

/// A widget whose header summary and body come straight from host state.
struct ReadWidget {
    host: Host,
    id: &'static str,
    title: &'static str,
    content: Content,
}

impl Render for ReadWidget {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app: &App = cx;
        let (summary, body) = self
            .host
            .read(app, |host| (self.content)(host, app))
            .unwrap_or_else(|| (None, muted("Нет данных.", app).into_any_element()));
        widget_frame(
            self.id,
            self.title.into(),
            summary,
            is_open(self.id, app).then_some(body),
            app,
        )
    }
}

fn read_widget(
    host: Host,
    id: &'static str,
    title: &'static str,
    content: Content,
    cx: &mut App,
) -> AnyView {
    cx.new(|_| ReadWidget {
        host,
        id,
        title,
        content,
    })
    .into()
}

pub(super) const CONTEXT: &str = "context";
pub(super) const PLAN: &str = "plan";
pub(super) const SESSION: &str = "session";
pub(super) const AGENT: &str = "agent";

pub(super) fn build_context(host: Host, _: &mut Window, cx: &mut App) -> AnyView {
    read_widget(host, CONTEXT, "Контекст", context, cx)
}

pub(super) fn build_plan(host: Host, _: &mut Window, cx: &mut App) -> AnyView {
    read_widget(host, PLAN, "План", plan, cx)
}

pub(super) fn build_session(host: Host, _: &mut Window, cx: &mut App) -> AnyView {
    read_widget(host, SESSION, "Сессия", session, cx)
}

pub(super) fn build_agent(host: Host, _: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|_| AgentWidget {
        host,
        show_tools: false,
    })
    .into()
}

fn context(host: &HostView, cx: &App) -> (Option<SharedString>, AnyElement) {
    let Some(usage) = host.session().and_then(|session| session.context) else {
        return (
            None,
            muted("Появится после первого запроса к модели.", cx).into_any_element(),
        );
    };
    let percent = usage.percent();
    let body = v_flex()
        .gap_2()
        .child(
            Progress::new("context-bar")
                .value(percent)
                .color(usage.color(cx.theme()))
                .xsmall(),
        )
        .child(row(
            "Занято",
            format!("{} из {}", tokens(usage.used), tokens(usage.max)),
            cx,
        ))
        .children(
            usage
                .trigger
                .map(|trigger| row("Автокомпакт", format!("при {}", tokens(trigger)), cx)),
        );
    (
        Some(format!("{percent:.0}%").into()),
        body.into_any_element(),
    )
}

/// Steps of the latest `update_plan` call of the root thread.
pub(super) fn current_plan(host: &HostView) -> Vec<PlanStep> {
    host.session()
        .and_then(|session| {
            session.transcript.entries().iter().rev().find_map(|entry| {
                entry
                    .tool
                    .as_ref()
                    .filter(|tool| tool.name == tool_display::UPDATE_PLAN)
            })
        })
        .map(|tool| tool_display::plan_steps(tool_args(tool)))
        .unwrap_or_default()
}

pub(super) fn completed(steps: &[PlanStep]) -> usize {
    steps
        .iter()
        .filter(|step| step.status == StepStatus::Completed)
        .count()
}

fn plan(host: &HostView, cx: &App) -> (Option<SharedString>, AnyElement) {
    let steps = current_plan(host);
    if steps.is_empty() {
        return (
            None,
            muted("Агент ещё не составил план.", cx).into_any_element(),
        );
    }
    (
        Some(format!("{}/{}", completed(&steps), steps.len()).into()),
        plan_list(&steps, cx).into_any_element(),
    )
}

pub(super) struct SessionStats {
    pub status: &'static str,
    pub attention: bool,
    pub messages: usize,
    pub tools: usize,
    pub waiting: usize,
    pub queued: usize,
}

pub(super) fn session_stats(host: &HostView) -> Option<SessionStats> {
    let session = host.session()?;
    let activity = host.activity().map(|activity| &activity.status);
    let status = match (session.connected, activity) {
        (false, _) => "Нет связи",
        (true, Some(AppSessionActivityStatus::Running)) => "Работает",
        (true, Some(AppSessionActivityStatus::WaitingApproval)) => "Ждёт подтверждения",
        (true, Some(AppSessionActivityStatus::WaitingInput)) => "Ждёт ответа",
        (true, _) if session.running() => "Работает",
        (true, _) => "Ожидает",
    };
    let entries = session.transcript.entries();
    let messages = entries
        .iter()
        .filter(|entry| entry.tool.is_none() && entry.subagent.is_none())
        .filter(|entry| matches!(entry.role, Role::User | Role::Assistant))
        .count();
    let tools = entries
        .iter()
        .map(|entry| {
            usize::from(entry.tool.is_some())
                + entry.subagent.as_ref().map_or(0, |agent| agent.tools.len())
        })
        .sum();
    let (waiting, queued) = session.pending.as_ref().map_or((0, 0), |pending| {
        (
            pending.approvals.len() + pending.user_inputs.len(),
            pending.queued_user_messages.len(),
        )
    });
    Some(SessionStats {
        status,
        attention: !session.connected
            || matches!(
                activity,
                Some(
                    AppSessionActivityStatus::WaitingApproval
                        | AppSessionActivityStatus::WaitingInput
                )
            ),
        messages,
        tools,
        waiting,
        queued,
    })
}

fn session(host: &HostView, cx: &App) -> (Option<SharedString>, AnyElement) {
    let Some(stats) = session_stats(host) else {
        return (None, muted("Сессия не открыта.", cx).into_any_element());
    };
    let body = v_flex()
        .gap_1()
        .child(row("Статус", stats.status, cx))
        .child(row("Сообщений", stats.messages.to_string(), cx))
        .child(row("Вызовов инструментов", stats.tools.to_string(), cx))
        .child(row("Ожидают ответа", stats.waiting.to_string(), cx))
        .child(row("В очереди", stats.queued.to_string(), cx));
    (Some(stats.status.into()), body.into_any_element())
}

pub(super) fn model_label(config: &ConfigSummary) -> String {
    config.model.as_ref().map_or("—".to_owned(), |model| {
        if model.label.is_empty() {
            model.name.clone()
        } else {
            model.label.clone()
        }
    })
}

/// Profile, model, mode and modules of the open session.
struct AgentWidget {
    host: Host,
    show_tools: bool,
}

impl AgentWidget {
    fn body(&self, host: &HostView, cx: &Context<Self>) -> AnyElement {
        let Some(config) = host.session().and_then(|session| session.config.as_ref()) else {
            return muted("Загрузка…", cx).into_any_element();
        };
        let model = model_label(config);
        let theme = cx.theme();
        let show_tools = self.show_tools;
        v_flex()
            .gap_1()
            .child(row("Профиль", config.profile.clone(), cx))
            .child(row("Модель", model, cx))
            .child(row("Режим", mode_label(&config.permission_mode), cx))
            .child(row(
                "Инструментов",
                config.registered_tools.len().to_string(),
                cx,
            ))
            .child(
                v_flex()
                    .pt_2()
                    .gap_0p5()
                    .children(config.modules.iter().map(|module| {
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(theme.muted_foreground)
                                    .child(module.slot.clone()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(theme.mono_font_family.clone())
                                    .child(module.id.clone()),
                            )
                    })),
            )
            .child(
                div().pt_2().child(
                    Button::new("panel-tools")
                        .ghost()
                        .xsmall()
                        .label(if show_tools {
                            "Скрыть инструменты"
                        } else {
                            "Показать инструменты"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_tools = !this.show_tools;
                            cx.notify();
                        })),
                ),
            )
            .children(show_tools.then(|| {
                v_flex()
                    .gap_0p5()
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .children(
                        config
                            .registered_tools
                            .iter()
                            .map(|tool| div().truncate().child(tool.name.clone())),
                    )
            }))
            .into_any_element()
    }
}

impl Render for AgentWidget {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if is_open(AGENT, cx) {
            self.host.read(cx, |host| self.body(host, cx))
        } else {
            None
        };
        let summary = self
            .host
            .read(cx, |host| {
                host.session()
                    .and_then(|session| session.config.as_ref())
                    .map(|config| SharedString::from(config.profile.clone()))
            })
            .flatten();
        widget_frame(AGENT, "Агент".into(), summary, body, cx)
    }
}
