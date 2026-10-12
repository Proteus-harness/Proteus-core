//! «Контекст и инструменты» tab: window map, cache, history, tools, compaction.

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppContextMapSnapshot;

use super::super::widgets::{Tone, badge, card, kv, metric, note};
use super::UsagePage;
use crate::analytics::analysis::compaction_tokens;
use crate::analytics::context::{
    CacheStatus, MAP_CELLS, MAP_COLUMNS, Segment, activity_label, allocate_cells, cache,
    max_tokens, segments, source_label, usage_title, used_tokens,
};
use crate::analytics::format::{short_id, tokens};
use crate::ui::prefs;

impl UsagePage {
    pub(super) fn render_context(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(snapshot) = &self.context.data else {
            return note(
                if self.context.loading {
                    "Загружаю контекст…"
                } else {
                    ""
                },
                cx,
            )
            .into_any_element();
        };
        let used = used_tokens(snapshot);
        let max = max_tokens(snapshot);
        let usage = snapshot.latest_usage.as_ref();
        let trigger = usage.and_then(|usage| usage.compaction_trigger_tokens);
        let segments = segments(
            usage
                .map(|usage| usage.categories.as_slice())
                .unwrap_or(&[]),
            used,
            max,
            trigger,
        );
        let cache = cache(usage);
        v_flex()
            .w_full()
            .gap_4()
            .child(note(
                "Последнее состояние сессии. Выбор хода в отчёте расхода не меняет этот снимок.",
                cx,
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(metric(
                        "занято",
                        tokens(used.into()),
                        Some(match max {
                            Some(max) => format!(
                                "{}% окна",
                                (f64::from(used) / f64::from(max) * 100.).round()
                            ),
                            None => "размер окна неизвестен".into(),
                        }),
                        cx,
                    ))
                    .child(metric(
                        "свободно",
                        max.map(|max| tokens(max.saturating_sub(used).into()))
                            .unwrap_or_else(|| "n/a".into()),
                        Some(match max {
                            Some(max) => format!("из {}", tokens(max.into())),
                            None => "max_input_tokens не задан".into(),
                        }),
                        cx,
                    ))
                    .child(metric(
                        "кэш-хиты",
                        cache
                            .hit_percent
                            .map(|hit| format!("{hit}%"))
                            .unwrap_or_else(|| "n/a".into()),
                        Some("входной кэш провайдера".into()),
                        cx,
                    ))
                    .child(metric(
                        "кэш",
                        cache.status.label(),
                        Some(cache.status.detail().into()),
                        cx,
                    )),
            )
            .child(
                card(usage_title(usage), cx)
                    .child(badge(source_label(usage), Tone::Muted, cx))
                    .child(render_map(&segments, cx)),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_start()
                    .child(panel(session_panel(snapshot, cx)))
                    .child(panel(history_panel(snapshot, cx)))
                    .child(panel(cache_panel(&cache, cx)))
                    .child(panel(tools_panel(snapshot, cx)))
                    .child(panel(compaction_panel(snapshot, cx)))
                    .child(panel(diagnostics_panel(snapshot, cx))),
            )
            .into_any_element()
    }
}

fn panel(content: Div) -> Div {
    div().flex_1().min_w(prefs::scaled(300.)).child(content)
}

fn hex(color: u32) -> Hsla {
    rgb(color).into()
}

fn render_map(segments: &[Segment], cx: &App) -> Div {
    if segments.is_empty() {
        return note("Для этой сессии ещё нет замеров использования", cx);
    }
    let counts = allocate_cells(
        &segments
            .iter()
            .map(|segment| segment.tokens)
            .collect::<Vec<_>>(),
        MAP_CELLS,
    );
    let mut cells = Vec::with_capacity(MAP_CELLS);
    for (segment, count) in segments.iter().zip(&counts) {
        cells.extend(std::iter::repeat_n(hex(segment.color), *count));
    }
    let size = prefs::scaled(14.);
    let rows = cells.chunks(MAP_COLUMNS).map(|row| {
        h_flex().gap(px(3.)).children(
            row.iter()
                .map(|color| div().size(size).rounded(cx.theme().radius / 2.).bg(*color)),
        )
    });
    h_flex()
        .gap_6()
        .items_start()
        .flex_wrap()
        .child(v_flex().gap(px(3.)).children(rows))
        .child(v_flex().gap_1().children(segments.iter().map(|segment| {
            h_flex()
                .gap_2()
                .items_center()
                .text_sm()
                .child(div().size_2p5().rounded_full().bg(hex(segment.color)))
                .child(div().w(prefs::scaled(170.)).child(segment.label.clone()))
                .child(
                    div()
                        .w(prefs::scaled(80.))
                        .child(tokens(segment.tokens.into())),
                )
                .child(note(format!("{:.1}%", segment.percent), cx))
        })))
}

fn session_panel(snapshot: &AppContextMapSnapshot, cx: &App) -> Div {
    let path = |path: &Option<std::path::PathBuf>| {
        path.as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "—".into())
    };
    card("Сессия", cx)
        .child(kv(
            "сессия",
            snapshot
                .session_id
                .as_ref()
                .map(short_id)
                .unwrap_or_else(|| "—".into()),
            cx,
        ))
        .child(kv("каталог", path(&snapshot.session_dir), cx))
        .child(kv("workspace", path(&snapshot.workspace_path), cx))
        .child(kv(
            "активность",
            activity_label(snapshot.activity.as_ref()),
            cx,
        ))
        .child(kv(
            "источник",
            source_label(snapshot.latest_usage.as_ref()),
            cx,
        ))
}

fn history_panel(snapshot: &AppContextMapSnapshot, cx: &App) -> Div {
    let history = &snapshot.history;
    let mut card = card("История", cx)
        .child(kv("сообщений", history.messages.to_string(), cx))
        .child(kv("от меня", history.user_messages.to_string(), cx))
        .child(kv("от агента", history.assistant_messages.to_string(), cx))
        .child(kv("tool-результаты", history.tool_results.to_string(), cx))
        .child(kv("оценка", tokens(history.estimated_tokens.into()), cx));
    if let Some(context) = &snapshot.latest_context {
        card = card
            .child(kv("чанков в ходе", context.chunks.to_string(), cx))
            .child(kv(
                "токенов хода",
                context
                    .token_estimate
                    .map(|t| tokens(t.into()))
                    .unwrap_or_else(|| "—".into()),
                cx,
            ));
    }
    card
}

fn cache_panel(cache: &crate::analytics::context::Cache, cx: &App) -> Div {
    let tone = match cache.status {
        CacheStatus::Hot => Tone::Success,
        CacheStatus::Warming => Tone::Warning,
        _ => Tone::Muted,
    };
    let value = |value: Option<u32>| {
        value
            .map(|value| tokens(value.into()))
            .unwrap_or_else(|| "n/a".into())
    };
    card("Кэш провайдера", cx)
        .child(badge(cache.status.label(), tone, cx))
        .child(kv("input", value(cache.input), cx))
        .child(kv("из кэша", value(cache.cached), cx))
        .child(kv("записано", value(cache.written), cx))
        .child(kv(
            "hit rate",
            cache
                .hit_percent
                .map(|hit| format!("{hit}%"))
                .unwrap_or_else(|| "n/a".into()),
            cx,
        ))
}

fn tools_panel(snapshot: &AppContextMapSnapshot, cx: &App) -> Div {
    let tools = &snapshot.tools;
    card("Инструменты", cx)
        .child(kv("запущено", tools.requested.to_string(), cx))
        .child(kv("завершено", tools.finished.to_string(), cx))
        .child(kv("с ошибкой", tools.failed.to_string(), cx))
        .child(if tools.names.is_empty() {
            note("нет tool-событий", cx)
        } else {
            h_flex().flex_wrap().gap_1().children(
                tools
                    .names
                    .iter()
                    .map(|name| badge(name.clone(), Tone::Muted, cx)),
            )
        })
}

fn compaction_panel(snapshot: &AppContextMapSnapshot, cx: &App) -> Div {
    let card = card("Компакция", cx);
    let Some(compaction) = &snapshot.latest_compaction else {
        return card.child(note("Сжатий не было", cx));
    };
    card.child(kv("status", compaction.status.clone(), cx))
        .child(kv(
            "summary",
            if compaction.summary_present {
                "сохранено, текст скрыт"
            } else {
                "нет текста"
            },
            cx,
        ))
        .when_some(compaction.report.as_ref(), |card, report| {
            card.child(kv("changed", report.changed.to_string(), cx))
                .child(kv(
                    "messages",
                    format!("{} → {}", report.input_messages, report.output_messages),
                    cx,
                ))
                .child(kv("tokens", compaction_tokens(report), cx))
        })
}

fn diagnostics_panel(snapshot: &AppContextMapSnapshot, cx: &App) -> Div {
    let theme = cx.theme();
    card("Диагностика", cx).children(if snapshot.diagnostics.is_empty() {
        vec![note("Нет предупреждений", cx)]
    } else {
        snapshot
            .diagnostics
            .iter()
            .map(|text| {
                div()
                    .text_sm()
                    .text_color(theme.warning)
                    .child(text.clone())
            })
            .collect()
    })
}
