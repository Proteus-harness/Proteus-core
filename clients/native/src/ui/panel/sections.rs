use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppSessionActivityStatus;

use super::{muted, row};
use crate::tool_display::{self, StepStatus};
use crate::transcript::Role;
use crate::ui::context_usage::tokens;
use crate::ui::controls::mode_label;
use crate::ui::tool_row::{plan_list, tool_args};
use crate::ui::workspace::Workspace;

impl Workspace {
    pub(super) fn context_section(&self, cx: &App) -> (Option<SharedString>, AnyElement) {
        let Some(usage) = self.session.as_ref().and_then(|session| session.context) else {
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
    pub(super) fn plan_section(&self, cx: &App) -> (Option<SharedString>, AnyElement) {
        let steps = self
            .session
            .as_ref()
            .and_then(|session| {
                session.transcript.entries().iter().rev().find_map(|entry| {
                    entry
                        .tool
                        .as_ref()
                        .filter(|tool| tool.name == tool_display::UPDATE_PLAN)
                })
            })
            .map(|tool| tool_display::plan_steps(tool_args(tool)))
            .unwrap_or_default();
        if steps.is_empty() {
            return (
                None,
                muted("Агент ещё не составил план.", cx).into_any_element(),
            );
        }
        let done = steps
            .iter()
            .filter(|step| step.status == StepStatus::Completed)
            .count();
        (
            Some(format!("{done}/{}", steps.len()).into()),
            plan_list(&steps, cx).into_any_element(),
        )
    }

    pub(super) fn session_section(&self, cx: &App) -> AnyElement {
        let Some(session) = self.session.as_ref() else {
            return muted("Сессия не открыта.", cx).into_any_element();
        };
        let activity = self
            .sessions
            .iter()
            .find(|summary| summary.session_dir == session.dir)
            .and_then(|summary| summary.activity.as_ref());
        let status = match (session.connected, activity.map(|activity| &activity.status)) {
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
        let tools: usize = entries
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
        v_flex()
            .gap_1()
            .child(row("Статус", status, cx))
            .child(row("Сообщений", messages.to_string(), cx))
            .child(row("Вызовов инструментов", tools.to_string(), cx))
            .child(row("Ожидают ответа", waiting.to_string(), cx))
            .child(row("В очереди", queued.to_string(), cx))
            .into_any_element()
    }

    pub(super) fn agent_section(&self, cx: &Context<Self>) -> AnyElement {
        let Some(config) = self
            .session
            .as_ref()
            .and_then(|session| session.config.as_ref())
        else {
            return muted("Загрузка…", cx).into_any_element();
        };
        let model = config.model.as_ref().map_or("—".to_owned(), |model| {
            if model.label.is_empty() {
                model.name.clone()
            } else {
                model.label.clone()
            }
        });
        let theme = cx.theme();
        let show_tools = self.panel.show_tools;
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
                            this.panel.show_tools = !this.panel.show_tools;
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
