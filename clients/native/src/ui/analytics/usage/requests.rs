//! «Запросы и расход» tab: period totals, per-model split and the request list.

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::contracts::ModelCallOrigin;
use proteus_contracts::domain::{ModelRequestUsage, ModelUsageStatus, SessionUsageSnapshot};

use super::super::widgets::{Choice, Tone, badge, card, kv, metric, note, select};
use super::{UsagePage, query_input};
use crate::analytics::format::{clock, money, seconds, tokens};
use crate::analytics::usage::{
    Cost, Order, PRICE_DATE, PRICE_SOURCE, Period, StatusFilter, Summary, by_model, request_cost,
    scoped, select as select_requests, status_label, summarize, turn_ids,
};
use crate::ui::kit;
use crate::ui::kit::Pressable as _;
use crate::ui::prefs;

const PAGE: usize = 20;

fn optional(value: Option<u64>) -> String {
    value.map(tokens).unwrap_or_else(|| "—".into())
}

fn status_tone(status: ModelUsageStatus) -> Tone {
    match status {
        ModelUsageStatus::Completed => Tone::Success,
        ModelUsageStatus::Unfinished => Tone::Warning,
        _ => Tone::Danger,
    }
}

impl UsagePage {
    pub(super) fn render_requests(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = match &self.usage.data {
            None => {
                return note(
                    if self.usage.loading {
                        "Загружаю расход…"
                    } else {
                        ""
                    },
                    cx,
                )
                .into_any_element();
            }
            Some(None) => {
                return note("Журнал расхода недоступен для этого чата.", cx).into_any_element();
            }
            Some(Some(snapshot)) => snapshot.clone(),
        };
        let turns = turn_ids(&snapshot);
        let mut scope = scoped(&snapshot, &self.period);
        let mut models: Vec<String> = Vec::new();
        for request in &snapshot.requests {
            let key = model_key(request);
            if !models.contains(&key) {
                models.push(key);
            }
        }
        if let Some(model) = &self.model {
            scope.retain(|index| model_key(&snapshot.requests[*index]) == *model);
        }
        let summary = summarize(scope.iter().map(|index| &snapshot.requests[*index]));
        let query = self.query.read(cx).value().to_string();
        let rows = select_requests(&snapshot, &scope, self.status, &query, self.order);
        let pages = rows.len().div_ceil(PAGE).max(1);
        self.page = self.page.min(pages - 1);
        let shown: Vec<usize> = rows
            .iter()
            .skip(self.page * PAGE)
            .take(PAGE)
            .copied()
            .collect();

        v_flex()
            .w_full()
            .gap_4()
            .child(self.render_controls(&turns, &models, cx))
            .child(self.render_summary(&summary, &scope, &snapshot, cx))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().w(prefs::scaled(320.)).child(query_input(&self.query)))
                    .child(select(
                        "usage-status",
                        self.status.label(),
                        StatusFilter::ALL
                            .iter()
                            .map(|status| Choice {
                                label: status.label().into(),
                                value: *status,
                                checked: *status == self.status,
                            })
                            .collect(),
                        {
                            let view = cx.entity().downgrade();
                            move |status, _, cx| {
                                view.update(cx, |this, cx| {
                                    this.status = status;
                                    this.page = 0;
                                    cx.notify();
                                })
                                .ok();
                            }
                        },
                    ))
                    .child(select(
                        "usage-order",
                        self.order.label(),
                        Order::ALL
                            .iter()
                            .map(|order| Choice {
                                label: order.label().into(),
                                value: *order,
                                checked: *order == self.order,
                            })
                            .collect(),
                        {
                            let view = cx.entity().downgrade();
                            move |order, _, cx| {
                                view.update(cx, |this, cx| {
                                    this.order = order;
                                    cx.notify();
                                })
                                .ok();
                            }
                        },
                    ))
                    .child(div().flex_1())
                    .child(note(
                        format!("Запросы: {} из {}", rows.len(), scope.len()),
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .when(shown.is_empty(), |list| {
                        list.child(note(
                            if snapshot.requests.is_empty() {
                                "В этой сессии ещё нет записанных запросов."
                            } else {
                                "По этим фильтрам запросов нет."
                            },
                            cx,
                        ))
                    })
                    .children(
                        shown
                            .iter()
                            .map(|index| self.render_request(&snapshot, &turns, *index, cx)),
                    ),
            )
            .when(pages > 1, |page| page.child(self.render_pages(pages, cx)))
            .child(note(
                format!(
                    "Оценка по стандартным ценам API ({PRICE_SOURCE}, {PRICE_DATE}), а не списание. Неизвестные модели не оцениваются."
                ),
                cx,
            ))
            .into_any_element()
    }

    fn render_controls(&self, turns: &[String], models: &[String], cx: &Context<Self>) -> Div {
        let period_label = match &self.period {
            Period::All => "Весь чат".to_string(),
            Period::Latest => "Последний ход".to_string(),
            Period::Turn(turn) => turns
                .iter()
                .position(|id| id == turn)
                .map(|index| format!("Ход {}", index + 1))
                .unwrap_or_else(|| "Ход".into()),
        };
        let mut periods = vec![
            Choice {
                label: "Весь чат".into(),
                value: Period::All,
                checked: self.period == Period::All,
            },
            Choice {
                label: "Последний ход".into(),
                value: Period::Latest,
                checked: self.period == Period::Latest,
            },
        ];
        for (index, turn) in turns.iter().enumerate().rev() {
            let value = Period::Turn(turn.clone());
            periods.push(Choice {
                label: format!("Ход {}", index + 1).into(),
                checked: self.period == value,
                value,
            });
        }
        let view = cx.entity().downgrade();
        let period = select(
            "usage-period",
            period_label,
            periods,
            move |period, _, cx| {
                view.update(cx, |this, cx| {
                    this.period = period;
                    this.page = 0;
                    cx.notify();
                })
                .ok();
            },
        );
        let model = (models.len() > 1).then(|| {
            let mut choices = vec![Choice {
                label: "Все модели".into(),
                value: None,
                checked: self.model.is_none(),
            }];
            choices.extend(models.iter().map(|model| Choice {
                label: model.clone().into(),
                value: Some(model.clone()),
                checked: self.model.as_ref() == Some(model),
            }));
            let view = cx.entity().downgrade();
            select(
                "usage-model",
                self.model.clone().unwrap_or_else(|| "Все модели".into()),
                choices,
                move |model, _, cx| {
                    view.update(cx, |this, cx| {
                        this.model = model;
                        this.page = 0;
                        cx.notify();
                    })
                    .ok();
                },
            )
        });
        h_flex().gap_2().child(period).children(model)
    }

    fn render_summary(
        &self,
        summary: &Summary,
        scope: &[usize],
        snapshot: &SessionUsageSnapshot,
        cx: &Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let mut warnings = Vec::new();
        if summary.reported < summary.requests {
            warnings.push(format!(
                "Токены известны для {} из {} запросов. Остальной расход неизвестен.",
                summary.reported, summary.requests
            ));
        }
        if summary.priced < summary.reported {
            warnings.push(format!(
                "Стоимость учтена для {} из {} запросов; показана известная часть.",
                summary.priced, summary.requests
            ));
        }
        let models = by_model(scope.iter().map(|index| &snapshot.requests[*index]));
        card("Итог выбранного периода", cx)
            .child(
                h_flex()
                    .gap_2()
                    .items_baseline()
                    .child(
                        div()
                            .text_2xl()
                            .font_semibold()
                            .child(tokens(summary.input + summary.output)),
                    )
                    .child(note("токенов", cx))
                    .child(
                        div()
                            .text_lg()
                            .text_color(theme.primary)
                            .child(format!("≈ {}", money(summary.cost))),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(metric("Вход", tokens(summary.input), None, cx))
                    .child(metric("Выход", tokens(summary.output), None, cx))
                    .child(metric(
                        "Из кэша",
                        optional(summary.cached),
                        Some("входит во вход".into()),
                        cx,
                    ))
                    .child(metric(
                        "Запись кэша",
                        optional(summary.write),
                        Some("входит во вход".into()),
                        cx,
                    ))
                    .child(metric(
                        "Рассуждения",
                        optional(summary.reasoning),
                        Some("входят в выход".into()),
                        cx,
                    ))
                    .child(metric(
                        "Запросы",
                        summary.requests.to_string(),
                        Some(
                            [
                                (summary.auxiliary > 0)
                                    .then(|| format!("вспомогательные: {}", summary.auxiliary)),
                                (summary.errors > 0).then(|| format!("ошибки: {}", summary.errors)),
                                (summary.unfinished > 0)
                                    .then(|| format!("без результата: {}", summary.unfinished)),
                            ]
                            .into_iter()
                            .flatten()
                            .collect::<Vec<_>>()
                            .join(" · "),
                        )
                        .filter(|detail| !detail.is_empty()),
                        cx,
                    )),
            )
            .children(
                warnings
                    .into_iter()
                    .map(|warning| div().text_xs().text_color(theme.warning).child(warning)),
            )
            .when(models.len() > 1, |card| {
                card.child(
                    v_flex()
                        .gap_1()
                        .children(models.into_iter().map(|(name, summary)| {
                            kv(
                                name,
                                format!(
                                    "{} · ≈ {}",
                                    tokens(summary.input + summary.output),
                                    money(summary.cost)
                                ),
                                cx,
                            )
                        })),
                )
            })
    }

    fn render_request(
        &self,
        snapshot: &SessionUsageSnapshot,
        turns: &[String],
        index: usize,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let request = &snapshot.requests[index];
        let open = self.expanded.contains(&index);
        let turn = request
            .turn_id
            .as_ref()
            .and_then(|turn| turns.iter().position(|id| *id == turn.to_string()))
            .map(|turn| format!("Ход {}", turn + 1))
            .unwrap_or_else(|| "Вне хода".into());
        let auxiliary = matches!(request.origin, ModelCallOrigin::Auxiliary);
        let cost = request_cost(request);
        let usage = request.usage.as_ref();
        let cell = |label: &'static str, value: Option<u32>| {
            v_flex()
                .w(prefs::scaled(84.))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .child(
                    div().text_sm().child(
                        value
                            .map(|v| tokens(v.into()))
                            .unwrap_or_else(|| "—".into()),
                    ),
                )
        };
        let total = usage
            .map(|usage| tokens(u64::from(usage.input_tokens) + u64::from(usage.output_tokens)))
            .unwrap_or_else(|| "—".into());
        v_flex()
            .id(("usage-request", index))
            .p_2()
            .gap_1()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .cursor_pointer()
            .pressable(cx)
            .hover(|row| row.bg(theme.list_hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.expanded.remove(&index) {
                    this.expanded.insert(index);
                }
                cx.notify();
            }))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .text_sm()
                    .child(div().font_semibold().child(format!(
                        "#{} · {}",
                        index + 1,
                        request.model.model
                    )))
                    .child(badge(
                        status_label(request.status),
                        status_tone(request.status),
                        cx,
                    ))
                    .when(auxiliary, |row| {
                        row.child(badge("Вспомогательный", Tone::Accent, cx))
                    })
                    .child(note(
                        format!(
                            "{} · {} · {}",
                            turn,
                            seconds(request.started_at_ms, request.finished_at_ms),
                            clock(request.started_at_ms)
                        ),
                        cx,
                    ))
                    .child(div().flex_1())
                    .child(div().font_semibold().child(total))
                    .child(note(
                        cost.total()
                            .map(|cost| format!("≈ {}", money(cost)))
                            .unwrap_or_else(|| "—".into()),
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(cell("Вход", usage.map(|u| u.input_tokens)))
                    .child(cell("Кэш", usage.and_then(|u| u.cached_input_tokens)))
                    .child(cell(
                        "Запись",
                        usage.and_then(|u| u.cache_creation_input_tokens),
                    ))
                    .child(cell("Выход", usage.map(|u| u.output_tokens)))
                    .child(cell("Мысли", usage.and_then(|u| u.reasoning_output_tokens))),
            )
            .when(open, |row| row.child(request_details(request, &cost, cx)))
            .into_any_element()
    }

    fn render_pages(&self, pages: usize, cx: &Context<Self>) -> Div {
        use gpui_kit::component::button::ButtonVariants as _;
        h_flex()
            .gap_2()
            .items_center()
            .child(
                kit::button("usage-prev")
                    .label("Назад")
                    .small()
                    .ghost()
                    .disabled(self.page == 0)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = this.page.saturating_sub(1);
                        cx.notify();
                    })),
            )
            .child(note(format!("{} / {pages}", self.page + 1), cx))
            .child(
                kit::button("usage-next")
                    .label("Дальше")
                    .small()
                    .ghost()
                    .disabled(self.page + 1 >= pages)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page += 1;
                        cx.notify();
                    })),
            )
    }
}

fn model_key(request: &ModelRequestUsage) -> String {
    format!("{} / {}", request.model.provider, request.model.model)
}

fn request_details(request: &ModelRequestUsage, cost: &Cost, cx: &App) -> Div {
    let mut rows = vec![
        kv("Провайдер", request.model.provider.clone(), cx),
        kv(
            "Тип",
            match request.origin {
                ModelCallOrigin::Auxiliary => "Вспомогательный запрос",
                ModelCallOrigin::Direct => "Запрос модели",
            },
            cx,
        ),
        kv("Начало", clock(request.started_at_ms), cx),
        kv(
            "Длительность",
            seconds(request.started_at_ms, request.finished_at_ms),
            cx,
        ),
        kv("Сообщений на входе", request.message_count.to_string(), cx),
        kv("Доступных инструментов", request.tool_count.to_string(), cx),
    ];
    if let Some(effort) = &request.reasoning_effort {
        rows.push(kv("Уровень рассуждения", effort.clone(), cx));
    }
    if let Some(limit) = request.max_output_tokens {
        rows.push(kv("Лимит выхода", tokens(limit.into()), cx));
    }
    if let Some(reason) = &request.finish_reason {
        rows.push(kv("Причина завершения", format!("{reason:?}"), cx));
    }
    match cost {
        Cost::Priced {
            input,
            output,
            long,
        } => {
            rows.push(kv("Стоимость входа", money(*input), cx));
            rows.push(kv("Стоимость выхода", money(*output), cx));
            if *long {
                rows.push(kv("Тариф", "Применён тариф длинного контекста", cx));
            }
        }
        Cost::Unpriced(reason) => rows.push(kv("Стоимость", *reason, cx)),
    }
    if request.usage.is_none() {
        rows.push(note(
            "Провайдер не вернул данные о токенах. Нулевой расход не предполагается.",
            cx,
        ));
    }
    v_flex()
        .pt_2()
        .gap_1()
        .border_t_1()
        .border_color(cx.theme().border)
        .children(rows)
        .child(kv("ID запроса", request.exchange_id.to_string(), cx))
        .children(
            request
                .turn_id
                .as_ref()
                .map(|turn| kv("ID хода", turn.to_string(), cx)),
        )
}
