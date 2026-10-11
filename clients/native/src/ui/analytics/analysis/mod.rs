//! «Анализ ходов»: saved turns of a session, their steps and step details.

mod details;

use crate::ui::kit;
use std::collections::HashSet;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::analysis::{
    AppAnalysisStep, AppAnalysisTurnStatus, AppSessionAnalysis, AppTurnAnalysis,
};

use super::picker::SessionPicker;
use super::widgets::{
    Fetch, Tone, badge, card, code, error_line, header, metric, note, refresh_button,
};
use crate::analytics::analysis::{
    default_step, duration, step_has_error, step_kind, step_status, step_title, turn_metrics,
    turn_status,
};
use crate::analytics::format::{clock, pretty, short_id, tokens};
use crate::api::Api;
use crate::ui::prefs;

pub(super) struct AnalysisPage {
    api: Api,
    picker: Entity<SessionPicker>,
    data: Fetch<AppSessionAnalysis>,
    /// Requested turn; `None` lets the server pick the latest.
    turn: Option<String>,
    step: Option<u64>,
    query: Entity<InputState>,
    problems: bool,
    /// Open disclosures in the step details.
    open: HashSet<String>,
    _subscriptions: Vec<Subscription>,
}

fn turn_tone(status: AppAnalysisTurnStatus) -> Tone {
    match status {
        AppAnalysisTurnStatus::Success => Tone::Success,
        AppAnalysisTurnStatus::Unsettled => Tone::Warning,
        _ => Tone::Danger,
    }
}

impl AnalysisPage {
    pub fn new(
        api: Api,
        picker: Entity<SessionPicker>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Название шага…"));
        let subscriptions = vec![
            cx.observe(&picker, |this, _, cx| {
                this.data.reset();
                this.turn = None;
                this.step = None;
                cx.notify();
            }),
            cx.subscribe(&query, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        Self {
            api,
            picker,
            data: Fetch::default(),
            turn: None,
            step: None,
            query,
            problems: false,
            open: HashSet::new(),
            _subscriptions: subscriptions,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let dir = self.picker.read(cx).selected.clone();
        let api = self.api.clone();
        let turn = self.turn.clone();
        self.data.start(
            move || api.analysis(&dir, turn.as_deref()),
            |this| &mut this.data,
            cx,
        );
    }

    fn select_turn(&mut self, turn: String, cx: &mut Context<Self>) {
        self.turn = Some(turn);
        self.step = None;
        self.refresh(cx);
        cx.notify();
    }

    pub(super) fn toggle(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.open.remove(&key) {
            self.open.insert(key);
        }
        cx.notify();
    }

    fn selected_step<'a>(&self, turn: &'a AppTurnAnalysis) -> Option<&'a AppAnalysisStep> {
        turn.steps
            .iter()
            .find(|step| Some(step.sequence) == self.step)
    }
}

impl Render for AnalysisPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.data.stale(None) {
            self.refresh(cx);
        }
        if let Some(selected) = self
            .data
            .data
            .as_ref()
            .and_then(|data| data.selected.as_ref())
            && self.selected_step(selected).is_none()
        {
            self.step = default_step(&selected.steps);
        }
        let mut page = v_flex().size_full().gap_3().child(header(
            "Анализ ходов",
            "От задачи до результата — по сохранённым фактам. Просмотр не запускает сессию и не меняет активный чат.",
            [
                SessionPicker::render(&self.picker, cx),
                refresh_button("analysis-refresh", self.data.loading, cx, Self::refresh),
            ],
            cx,
        ));
        if let Some(error) = &self.data.error {
            page = page.child(error_line(
                format!("Не удалось загрузить анализ: {error}"),
                cx,
            ));
        }
        let Some(data) = self.data.data.clone() else {
            return page.child(note(
                if self.data.loading {
                    "Читаю журнал…"
                } else {
                    ""
                },
                cx,
            ));
        };
        page.child(note(
            format!(
                "Сессия {} · версия журнала {}",
                short_id(data.session_id),
                data.revision
            ),
            cx,
        ))
        .child(
            h_flex()
                .flex_1()
                .min_h_0()
                .gap_3()
                .items_start()
                .child(self.render_turns(&data, cx))
                .child(match &data.selected {
                    Some(turn) => self.render_turn(turn, cx).into_any_element(),
                    None => note("В журнале пока нет ходов.", cx).into_any_element(),
                }),
        )
    }
}

impl AnalysisPage {
    fn render_turns(&self, data: &AppSessionAnalysis, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let current = data.selected.as_ref().map(|turn| turn.turn_id.to_string());
        v_flex()
            .id("analysis-turns")
            .w(prefs::scaled(240.))
            .flex_none()
            .h_full()
            .gap_1()
            .overflow_y_scroll()
            .child(div().text_xs().font_semibold().child("Ходы"))
            .children(data.turns.iter().rev().map(|turn| {
                let id = turn.turn_id.to_string();
                let selected = current.as_deref() == Some(id.as_str());
                v_flex()
                    .id(SharedString::from(format!("analysis-turn-{id}")))
                    .p_2()
                    .gap_0p5()
                    .rounded(theme.radius)
                    .cursor_pointer()
                    .when(selected, |row| row.bg(theme.accent))
                    .when(!selected, |row| row.hover(|row| row.bg(theme.list_hover)))
                    .child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .child(div().font_semibold().child(format!("Ход {}", turn.number)))
                            .child(badge(turn_status(turn.status), turn_tone(turn.status), cx)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .line_clamp(2)
                            .child(turn.prompt_preview.clone()),
                    )
                    .child(note(clock(turn.started_at_ms), cx).text_xs())
                    .on_click(cx.listener(move |this, _, _, cx| this.select_turn(id.clone(), cx)))
            }))
    }

    fn render_turn(&self, turn: &AppTurnAnalysis, cx: &Context<Self>) -> impl IntoElement {
        let metrics = turn_metrics(&turn.steps);
        let status = turn
            .steps
            .first()
            .zip(turn.steps.last())
            .map(|(first, last)| duration(first.started_at_ms, last.finished_at_ms))
            .unwrap_or_else(|| "—".into());
        let query = self.query.read(cx).value().trim().to_lowercase();
        let steps: Vec<&AppAnalysisStep> = turn
            .steps
            .iter()
            .filter(|step| {
                (!self.problems || step_has_error(step) || step_status(step) == "Нет результата")
                    && (query.is_empty() || step_title(step).to_lowercase().contains(&query))
            })
            .collect();
        let theme = cx.theme();
        let summary = card("Ход", cx)
            .child(div().text_sm().child(turn.task.text.clone()))
            .child(note(format!("Длительность шагов: {status}"), cx))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(metric(
                        "Запросы модели",
                        metrics.models.to_string(),
                        None,
                        cx,
                    ))
                    .child(metric("Инструменты", metrics.tools.to_string(), None, cx))
                    .child(metric("Ошибки шагов", metrics.errors.to_string(), None, cx))
                    .child(metric(
                        "Без результата",
                        metrics.missing.to_string(),
                        None,
                        cx,
                    ))
                    .child(metric(
                        "Токены: вход / выход",
                        if metrics.with_usage == 0 {
                            "нет данных".into()
                        } else {
                            format!("{} / {}", tokens(metrics.input), tokens(metrics.output))
                        },
                        Some(format!(
                            "usage у {} из {} запросов",
                            metrics.with_usage, metrics.models
                        )),
                        cx,
                    )),
            )
            .children(turn.error.as_ref().map(|error| {
                div()
                    .text_sm()
                    .text_color(theme.danger)
                    .child(format!("Причина завершения: {error}"))
            }))
            .child(
                self.disclosure(
                    "turn-output".into(),
                    "Итоговый ответ",
                    turn.output
                        .as_ref()
                        .map(|output| output.text.clone())
                        .unwrap_or_else(|| "Итоговый ответ не записан.".into()),
                    cx,
                ),
            )
            .child(self.disclosure(
                "turn-config".into(),
                &format!("Конфигурация при запуске · epoch {}", turn.module_epoch),
                pretty(&turn.config_snapshot),
                cx,
            ));
        let list = v_flex()
            .id("analysis-steps")
            .w(prefs::scaled(300.))
            .flex_none()
            .h_full()
            .gap_1()
            .overflow_y_scroll()
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .child(format!("Шаги · {}", turn.steps.len())),
            )
            .child(Input::new(&self.query).small().cleanable(true))
            .child(
                kit::checkbox("analysis-problems")
                    .label("Ошибки и без результата")
                    .checked(self.problems)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.problems = *checked;
                        cx.notify();
                    })),
            )
            .when(steps.is_empty(), |list| {
                list.child(note("Подходящих шагов нет.", cx))
            })
            .children(steps.iter().map(|step| {
                let selected = Some(step.sequence) == self.step;
                let sequence = step.sequence;
                let error = step_has_error(step);
                v_flex()
                    .id(("analysis-step", sequence as usize))
                    .p_2()
                    .rounded(theme.radius)
                    .cursor_pointer()
                    .when(selected, |row| row.bg(theme.accent))
                    .when(!selected, |row| row.hover(|row| row.bg(theme.list_hover)))
                    .child(note(format!("{} · #{}", step_kind(step), sequence), cx).text_xs())
                    .child(div().text_sm().truncate().child(step_title(step)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(if error {
                                theme.danger
                            } else {
                                theme.muted_foreground
                            })
                            .child(format!(
                                "{} · {}",
                                step_status(step),
                                duration(step.started_at_ms, step.finished_at_ms)
                            )),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.step = Some(sequence);
                        cx.notify();
                    }))
            }));
        let detail = self
            .selected_step(turn)
            .map(|step| self.render_step(step, cx).into_any_element())
            .unwrap_or_else(|| note("Выберите шаг.", cx).into_any_element());
        h_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .gap_3()
            .items_start()
            .child(list)
            .child(
                div()
                    .id("analysis-detail")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(v_flex().gap_3().child(summary).child(detail))
                    .overflow_y_scrollbar(),
            )
    }

    /// Clickable heading that reveals a code block.
    pub(super) fn disclosure(
        &self,
        key: String,
        title: &str,
        body: String,
        cx: &Context<Self>,
    ) -> AnyElement {
        let open = self.open.contains(&key);
        let theme = cx.theme();
        let toggle = key.clone();
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .id(SharedString::from(format!("analysis-open-{key}")))
                    .gap_1()
                    .items_center()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .cursor_pointer()
                    .hover(|row| row.text_color(theme.foreground))
                    .child(
                        Icon::new(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .xsmall(),
                    )
                    .child(title.to_string())
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle.clone(), cx))),
            )
            .when(open, |block| {
                block.child(code(
                    SharedString::from(format!("analysis-code-{key}")),
                    body,
                    cx,
                ))
            })
            .into_any_element()
    }
}
