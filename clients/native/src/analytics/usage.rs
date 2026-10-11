//! Request usage report: period/model selection, filters, totals and the
//! API-equivalent cost estimate (ported from the web usage extension).

use proteus_contracts::contracts::ModelCallOrigin;
use proteus_contracts::domain::{ModelRequestUsage, ModelUsageStatus, SessionUsageSnapshot};

pub const PRICE_DATE: &str = "2026-09-10";
pub const PRICE_SOURCE: &str = "https://developers.openai.com/api/docs/pricing";

/// Prices in USD per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rate {
    pub model: &'static str,
    pub input: f64,
    pub cached: f64,
    pub write: f64,
    pub output: f64,
    pub threshold: u64,
    pub input_multiplier: f64,
    pub output_multiplier: f64,
}

const fn rate(model: &'static str, input: f64, cached: f64, write: f64, output: f64) -> Rate {
    Rate {
        model,
        input,
        cached,
        write,
        output,
        threshold: 272_000,
        input_multiplier: 2.,
        output_multiplier: 1.5,
    }
}

pub const BUILTIN_RATES: [Rate; 5] = [
    rate("gpt-6-astra", 10., 1., 12.5, 50.),
    rate("gpt-5.6-sol", 4., 0.4, 5., 20.),
    rate("gpt-5.6", 4., 0.4, 5., 20.),
    rate("gpt-5.6-terra", 2., 0.2, 2.5, 12.),
    rate("gpt-5.6-luna", 0.2, 0.02, 0.25, 1.2),
];

#[derive(Clone, Debug, PartialEq)]
pub enum Cost {
    Priced { input: f64, output: f64, long: bool },
    Unpriced(&'static str),
}

impl Cost {
    pub fn total(&self) -> Option<f64> {
        match self {
            Self::Priced { input, output, .. } => Some(input + output),
            Self::Unpriced(_) => None,
        }
    }
}

pub fn request_cost(request: &ModelRequestUsage) -> Cost {
    let Some(usage) = &request.usage else {
        return Cost::Unpriced("Нет данных о токенах");
    };
    let Some(rate) = BUILTIN_RATES
        .iter()
        .find(|rate| rate.model == request.model.model)
    else {
        return Cost::Unpriced("Тариф не задан");
    };
    let input = u64::from(usage.input_tokens);
    let cached = u64::from(usage.cached_input_tokens.unwrap_or(0));
    let write = u64::from(usage.cache_creation_input_tokens.unwrap_or(0));
    if cached + write > input {
        return Cost::Unpriced("Некорректная разбивка токенов");
    }
    let long = rate.threshold > 0 && input > rate.threshold;
    let fresh = (input - cached - write) as f64;
    let input_cost = (fresh * rate.input + cached as f64 * rate.cached + write as f64 * rate.write)
        * if long { rate.input_multiplier } else { 1. }
        / 1e6;
    let output_cost = f64::from(usage.output_tokens)
        * rate.output
        * if long { rate.output_multiplier } else { 1. }
        / 1e6;
    Cost::Priced {
        input: input_cost,
        output: output_cost,
        long,
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub requests: usize,
    pub reported: usize,
    pub input: u64,
    pub output: u64,
    pub cached: Option<u64>,
    pub write: Option<u64>,
    pub reasoning: Option<u64>,
    pub cost: f64,
    pub priced: usize,
    pub unfinished: usize,
    pub errors: usize,
    pub auxiliary: usize,
}

fn add(slot: &mut Option<u64>, value: Option<u32>) {
    if let Some(value) = value {
        *slot = Some(slot.unwrap_or(0) + u64::from(value));
    }
}

pub fn summarize<'a>(requests: impl IntoIterator<Item = &'a ModelRequestUsage>) -> Summary {
    let mut summary = Summary::default();
    for request in requests {
        summary.requests += 1;
        match request.status {
            ModelUsageStatus::Unfinished => summary.unfinished += 1,
            ModelUsageStatus::Completed => {}
            _ => summary.errors += 1,
        }
        if matches!(request.origin, ModelCallOrigin::Auxiliary) {
            summary.auxiliary += 1;
        }
        if let Some(usage) = &request.usage {
            summary.reported += 1;
            summary.input += u64::from(usage.input_tokens);
            summary.output += u64::from(usage.output_tokens);
            add(&mut summary.cached, usage.cached_input_tokens);
            add(&mut summary.write, usage.cache_creation_input_tokens);
            add(&mut summary.reasoning, usage.reasoning_output_tokens);
        }
        if let Some(cost) = request_cost(request).total() {
            summary.cost += cost;
            summary.priced += 1;
        }
    }
    summary
}

pub fn status_label(status: ModelUsageStatus) -> &'static str {
    match status {
        ModelUsageStatus::Completed => "Готово",
        ModelUsageStatus::Error => "Ошибка",
        ModelUsageStatus::Canceled => "Отменён",
        ModelUsageStatus::Timeout => "Таймаут",
        ModelUsageStatus::Unfinished => "Нет результата",
    }
}

/// Which part of the session the report covers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Period {
    #[default]
    All,
    Latest,
    Turn(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusFilter {
    #[default]
    All,
    Problems,
    MissingUsage,
}

impl StatusFilter {
    pub const ALL: [Self; 3] = [Self::All, Self::Problems, Self::MissingUsage];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "Все статусы",
            Self::Problems => "Проблемные",
            Self::MissingUsage => "Без данных о токенах",
        }
    }

    fn keeps(self, request: &ModelRequestUsage) -> bool {
        match self {
            Self::All => true,
            Self::Problems => request.status != ModelUsageStatus::Completed,
            Self::MissingUsage => request.usage.is_none(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Order {
    #[default]
    Oldest,
    Newest,
    Longest,
    Tokens,
    Cost,
}

impl Order {
    pub const ALL: [Self; 5] = [
        Self::Oldest,
        Self::Newest,
        Self::Longest,
        Self::Tokens,
        Self::Cost,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Oldest => "По порядку",
            Self::Newest => "Сначала последние",
            Self::Longest => "Сначала долгие",
            Self::Tokens => "Больше токенов",
            Self::Cost => "Сначала дороже",
        }
    }
}

/// Turn ids in the order they first appear.
pub fn turn_ids(snapshot: &SessionUsageSnapshot) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for request in &snapshot.requests {
        if let Some(turn) = &request.turn_id {
            let turn = turn.to_string();
            if !ids.contains(&turn) {
                ids.push(turn);
            }
        }
    }
    ids
}

/// Requests of the period, as indexes into `snapshot.requests`.
pub fn scoped(snapshot: &SessionUsageSnapshot, period: &Period) -> Vec<usize> {
    let turn = match period {
        Period::All => None,
        Period::Latest => Some(snapshot.latest_turn_id.as_ref().map(ToString::to_string)),
        Period::Turn(turn) => Some(Some(turn.clone())),
    };
    snapshot
        .requests
        .iter()
        .enumerate()
        .filter(|(_, request)| match &turn {
            None => true,
            Some(turn) => request.turn_id.as_ref().map(ToString::to_string) == *turn,
        })
        .map(|(index, _)| index)
        .collect()
}

/// Filters and sorts the scoped requests; unknown values sort last.
pub fn select(
    snapshot: &SessionUsageSnapshot,
    scope: &[usize],
    status: StatusFilter,
    query: &str,
    order: Order,
) -> Vec<usize> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut rows: Vec<usize> = scope
        .iter()
        .copied()
        .filter(|index| {
            let request = &snapshot.requests[*index];
            if !status.keeps(request) {
                return false;
            }
            let haystack = format!(
                "{} {} {} {}",
                request.exchange_id,
                request
                    .turn_id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                request.model.provider,
                request.model.model
            )
            .to_lowercase();
            terms.iter().all(|term| haystack.contains(term))
        })
        .collect();
    let key = |index: &usize| -> Option<f64> {
        let request = &snapshot.requests[*index];
        match order {
            Order::Oldest | Order::Newest => Some(request.started_at_ms as f64),
            Order::Longest => request
                .finished_at_ms
                .map(|end| (end - request.started_at_ms) as f64),
            Order::Tokens => request
                .usage
                .as_ref()
                .map(|usage| f64::from(usage.input_tokens) + f64::from(usage.output_tokens)),
            Order::Cost => request_cost(request).total(),
        }
    };
    if order != Order::Oldest {
        rows.sort_by(|left, right| match (key(left), key(right)) {
            (Some(left), Some(right)) => right.total_cmp(&left),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
    }
    rows
}

/// Totals per `provider / model`, largest token use first.
pub fn by_model<'a>(
    requests: impl IntoIterator<Item = &'a ModelRequestUsage>,
) -> Vec<(String, Summary)> {
    let mut groups: Vec<(String, Vec<&ModelRequestUsage>)> = Vec::new();
    for request in requests {
        let key = format!("{} / {}", request.model.provider, request.model.model);
        match groups.iter_mut().find(|(name, _)| *name == key) {
            Some((_, list)) => list.push(request),
            None => groups.push((key, vec![request])),
        }
    }
    let mut out: Vec<(String, Summary)> = groups
        .into_iter()
        .map(|(name, list)| (name, summarize(list)))
        .collect();
    out.sort_by_key(|(_, summary)| std::cmp::Reverse(summary.input + summary.output));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot() -> SessionUsageSnapshot {
        let request =
            |id: &str, turn: &str, model: &str, status: &str, usage: serde_json::Value| {
                json!({
                    "exchange_id": id, "turn_id": turn,
                    "model": {"provider": "openai", "model": model},
                    "origin": "direct", "started_at_ms": 1000, "finished_at_ms": 3000,
                    "status": status, "finish_reason": null, "usage": usage,
                    "message_count": 2, "tool_count": 1, "reasoning_effort": null,
                    "max_output_tokens": null
                })
            };
        let turn_a = "11111111-1111-1111-1111-111111111111";
        let turn_b = "22222222-2222-2222-2222-222222222222";
        let exchange = |n: u8| format!("{n}{n}{n}{n}{n}{n}{n}{n}-0000-0000-0000-000000000000");
        serde_json::from_value(json!({
            "session_id": "00000000-0000-0000-0000-000000000000",
            "revision": 3,
            "latest_turn_id": turn_b,
            "requests": [
                request(&exchange(1), turn_a, "gpt-5.6", "completed",
                    json!({"input_tokens": 1_000_000, "output_tokens": 1000, "cached_input_tokens": 500_000, "cache_creation_input_tokens": null, "reasoning_output_tokens": 10})),
                request(&exchange(2), turn_b, "local", "error", json!(null)),
                request(&exchange(3), turn_b, "gpt-5.6", "completed",
                    json!({"input_tokens": 100, "output_tokens": 10, "cached_input_tokens": null, "cache_creation_input_tokens": null, "reasoning_output_tokens": null})),
            ]
        }))
        .expect("usage snapshot")
    }

    #[test]
    fn prices_long_context_and_skips_unknown_models() {
        let snapshot = snapshot();
        let Cost::Priced {
            input,
            output,
            long,
        } = request_cost(&snapshot.requests[0])
        else {
            panic!("priced");
        };
        assert!(long);
        assert!((input - (500_000. * 4. + 500_000. * 0.4) * 2. / 1e6).abs() < 1e-9);
        assert!((output - 1000. * 20. * 1.5 / 1e6).abs() < 1e-9);
        assert_eq!(
            request_cost(&snapshot.requests[1]),
            Cost::Unpriced("Нет данных о токенах")
        );
    }

    #[test]
    fn summarizes_and_selects_periods() {
        let snapshot = snapshot();
        let all = summarize(&snapshot.requests);
        assert_eq!((all.requests, all.reported, all.errors), (3, 2, 1));
        assert_eq!(all.cached, Some(500_000));
        assert_eq!(all.write, None);
        assert_eq!(scoped(&snapshot, &Period::Latest), vec![1, 2]);
        assert_eq!(turn_ids(&snapshot).len(), 2);
        let scope = scoped(&snapshot, &Period::All);
        assert_eq!(
            select(&snapshot, &scope, StatusFilter::Problems, "", Order::Oldest),
            vec![1]
        );
        assert_eq!(
            select(&snapshot, &scope, StatusFilter::All, "", Order::Tokens),
            vec![0, 2, 1]
        );
        assert_eq!(
            select(&snapshot, &scope, StatusFilter::All, "LOCAL", Order::Oldest),
            vec![1]
        );
        assert_eq!(by_model(&snapshot.requests)[0].0, "openai / gpt-5.6");
    }
}
