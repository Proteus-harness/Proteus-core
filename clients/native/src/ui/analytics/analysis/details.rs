//! Details of one analysis step by kind: model, tool, compaction, hooks.

use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::app_protocol::analysis::{AppAnalysisStep, AppAnalysisStepData};
use proteus_contracts::contracts::HookStepOutcome;

use super::super::widgets::{Tone, badge, card, kv, note};
use super::AnalysisPage;
use crate::analytics::analysis::{
    compaction_tokens, duration, message_text, step_has_error, step_kind, step_status, step_title,
};
use crate::analytics::format::{clock, pretty, tokens};

fn preview(text: &str) -> String {
    let mut out: String = text.chars().take(90).collect();
    if out.len() < text.len() {
        out.push('…');
    }
    out.replace('\n', " ")
}

impl AnalysisPage {
    pub(super) fn render_step(&self, step: &AppAnalysisStep, cx: &Context<Self>) -> Div {
        let key = |name: &str| format!("{}-{name}", step.sequence);
        let tone = if step_has_error(step) {
            Tone::Danger
        } else {
            Tone::Muted
        };
        let mut card = card(format!("{} · {}", step_kind(step), step_title(step)), cx).child(
            h_flex()
                .gap_2()
                .child(badge(step_status(step), tone, cx))
                .child(note(
                    format!(
                        "{} · {}",
                        duration(step.started_at_ms, step.finished_at_ms),
                        clock(step.started_at_ms)
                    ),
                    cx,
                )),
        );
        match &step.data {
            AppAnalysisStepData::Model {
                exchange_id,
                request,
                response,
                failure,
                messages,
                ..
            } => {
                if let Some(usage) = response.as_ref().and_then(|r| r.usage.as_ref()) {
                    card = card.child(kv(
                        "Токены",
                        format!(
                            "вход {} · выход {}{}",
                            tokens(usage.input_tokens.into()),
                            tokens(usage.output_tokens.into()),
                            usage
                                .cached_input_tokens
                                .map(|cached| format!(" · из кэша {}", tokens(cached.into())))
                                .unwrap_or_default()
                        ),
                        cx,
                    ));
                }
                if let Some(response) = response {
                    card = card.child(kv(
                        "Причина остановки",
                        format!(
                            "{:?} · конец хода: {}",
                            response.finish_reason,
                            match response.end_turn {
                                Some(true) => "да",
                                Some(false) => "нет",
                                None => "не указан",
                            }
                        ),
                        cx,
                    ));
                }
                card = card.child(kv("exchange", exchange_id.to_string(), cx));
                if let Some(failure) = failure {
                    card = card.child(div().text_sm().text_color(cx.theme().danger).child(
                        format!("Ошибка модели: {:?} — {}", failure.kind, failure.message),
                    ));
                }
                card = card.child(self.disclosure(
                    key("instructions"),
                    &format!("Инструкции · {}", request.instructions.len()),
                    pretty(&request.instructions),
                    cx,
                ));
                card = card.child(note(
                    format!("Сообщения запроса · {}", request.messages.len()),
                    cx,
                ));
                for (index, message) in request.messages.iter().enumerate() {
                    let text = message_text(message);
                    card = card.child(self.disclosure(
                        key(&format!("message-{index}")),
                        &format!("{:?} · {}", message.role, preview(&text)),
                        text,
                        cx,
                    ));
                }
                card = card.child(
                    self.disclosure(
                        key("tools"),
                        &format!("Доступные инструменты · {}", request.tools.len()),
                        request
                            .tools
                            .iter()
                            .map(|tool| tool.name.clone())
                            .collect::<Vec<_>>()
                            .join("\n"),
                        cx,
                    ),
                );
                let answer = response
                    .as_ref()
                    .map(|response| response.messages.as_slice())
                    .unwrap_or(messages.as_slice());
                let answer_text = answer
                    .iter()
                    .map(message_text)
                    .collect::<Vec<_>>()
                    .join("\n\n");
                card = card
                    .child(self.disclosure(
                        key("answer"),
                        "Записанный ответ",
                        if answer_text.is_empty() {
                            "Ответ не записан.".into()
                        } else {
                            answer_text
                        },
                        cx,
                    ))
                    .child(self.disclosure(
                        key("request"),
                        "Полный сохранённый запрос",
                        pretty(request),
                        cx,
                    ))
                    .child(self.disclosure(
                        key("response"),
                        "Полный сохранённый ответ",
                        pretty(response),
                        cx,
                    ));
            }
            AppAnalysisStepData::Tool {
                call,
                approval_reason,
                resolution,
                result,
            } => {
                card = card
                    .child(kv("call", call.id.to_string(), cx))
                    .child(self.disclosure(key("args"), "Аргументы", pretty(&call.args), cx));
                if let Some(reason) = approval_reason {
                    card = card.child(kv("Запрос подтверждения", reason.clone(), cx));
                }
                if let Some(resolution) = resolution {
                    card = card.child(self.disclosure(
                        key("resolution"),
                        "Решение о выполнении",
                        pretty(resolution),
                        cx,
                    ));
                }
                match result {
                    Some(result) => {
                        if let Some(error) = &result.error {
                            card = card.child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().danger)
                                    .child(error.clone()),
                            );
                        }
                        card = card
                            .child(self.disclosure(
                                key("output"),
                                "Результат",
                                result.output.clone(),
                                cx,
                            ))
                            .child(self.disclosure(
                                key("result"),
                                "Полный результат",
                                pretty(result),
                                cx,
                            ));
                    }
                    None => {
                        card = card.child(note(
                            "Результат не записан. Это не значит, что внешнего эффекта не было.",
                            cx,
                        ))
                    }
                }
            }
            AppAnalysisStepData::Compaction {
                report,
                history_revision,
            } => {
                card = card
                    .child(kv(
                        "Сообщения",
                        format!(
                            "{} → {} · версия истории {history_revision}",
                            report.input_messages, report.output_messages
                        ),
                        cx,
                    ))
                    .child(kv("Токены", compaction_tokens(report), cx));
                if let Some(reason) = &report.reason {
                    card = card.child(kv("Причина", reason.clone(), cx));
                }
                if let Some(summary) = &report.summary {
                    card =
                        card.child(self.disclosure(key("summary"), "Сводка", summary.clone(), cx));
                }
                card =
                    card.child(self.disclosure(key("report"), "Отчёт сжатия", pretty(report), cx));
            }
            AppAnalysisStepData::Hook { trace } => {
                card = card.child(note("Обработчики", cx));
                for hook in &trace.steps {
                    let (label, tone) = match &hook.outcome {
                        HookStepOutcome::Accepted { .. } => ("принят".to_string(), Tone::Success),
                        HookStepOutcome::Failed { message } => (message.clone(), Tone::Danger),
                    };
                    card = card.child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .child(hook.module_id.clone())
                            .child(badge(label, tone, cx)),
                    );
                }
                card = card
                    .child(self.disclosure(
                        key("input"),
                        "Входные данные",
                        pretty(&trace.input),
                        cx,
                    ))
                    .child(self.disclosure(
                        key("chain"),
                        "Результат цепочки",
                        pretty(&trace.output),
                        cx,
                    ));
            }
        }
        card.child(note(
            format!(
                "Запись {} · execution {} · thread {}",
                step.sequence,
                step.execution_id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "—".into()),
                step.thread_id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "—".into())
            ),
            cx,
        ))
    }
}
