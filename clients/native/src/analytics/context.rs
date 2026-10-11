//! Context window snapshot: used/free tokens, the 200-cell window map and
//! the provider cache state (ported from the web context map).

use proteus_contracts::app_protocol::{
    AppContextMapSnapshot, AppContextUsageCategory, AppContextUsageSnapshot, AppSessionActivity,
};

pub const MAP_CELLS: usize = 200;
pub const MAP_COLUMNS: usize = 20;

const PALETTE: [u32; 6] = [0x6b9eff, 0x5cc784, 0xd8a21e, 0xc08cf0, 0x5cc0d8, 0xef8fb7];
pub const FREE_COLOR: u32 = 0x2e2e2e;
pub const BUFFER_COLOR: u32 = 0x54383d;

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub label: String,
    pub tokens: u32,
    /// `0xRRGGBB`.
    pub color: u32,
    pub percent: f64,
}

pub fn used_tokens(snapshot: &AppContextMapSnapshot) -> u32 {
    let usage = snapshot.latest_usage.as_ref();
    usage
        .and_then(|usage| usage.actual.as_ref().map(|actual| actual.input_tokens))
        .or_else(|| usage.map(|usage| usage.estimated_input_tokens))
        .unwrap_or(snapshot.history.estimated_tokens)
}

pub fn max_tokens(snapshot: &AppContextMapSnapshot) -> Option<u32> {
    snapshot
        .latest_usage
        .as_ref()
        .and_then(|usage| usage.max_input_tokens)
        .filter(|max| *max > 0)
}

pub fn usage_title(usage: Option<&AppContextUsageSnapshot>) -> String {
    let Some(usage) = usage else {
        return "оценка по истории".into();
    };
    let phase = usage.phase.as_deref().unwrap_or("request");
    format!("{}/{} · {phase}", usage.model_provider, usage.model_name)
}

pub fn source_label(usage: Option<&AppContextUsageSnapshot>) -> String {
    let Some(usage) = usage else {
        return "history fallback".into();
    };
    match usage.source.as_str() {
        "mixed" => "provider totals + local estimates".into(),
        "provider" => "provider totals".into(),
        "estimated" => "local estimate".into(),
        other => other.into(),
    }
}

pub fn activity_label(activity: Option<&AppSessionActivity>) -> String {
    let Some(activity) = activity else {
        return "cold".into();
    };
    let status = activity.status.as_str();
    if activity.running_runs > 0 {
        format!("{status} · {} runs", activity.running_runs)
    } else if activity.pending_approvals > 0 {
        format!("{status} · approvals")
    } else if activity.pending_user_inputs > 0 {
        format!("{status} · input")
    } else {
        status.into()
    }
}

/// Content categories scaled to the actual input, then free space and the
/// auto-compaction reserve at the tail of the window.
pub fn segments(
    categories: &[AppContextUsageCategory],
    used: u32,
    max: Option<u32>,
    trigger: Option<u32>,
) -> Vec<Segment> {
    // Provider cache categories overlap ordinary input; they belong to the cache panel.
    let content: Vec<&AppContextUsageCategory> = categories
        .iter()
        .filter(|category| category.tokens > 0 && !category.name.starts_with("provider_cache"))
        .collect();
    let total: u64 = content.iter().map(|c| u64::from(c.tokens)).sum();
    let mut out: Vec<Segment> = content
        .iter()
        .enumerate()
        .map(|(index, category)| Segment {
            label: category_label(&category.name),
            tokens: (u64::from(used) * u64::from(category.tokens))
                .checked_div(total)
                .unwrap_or(0) as u32,
            color: category_color(&category.name, index),
            percent: 0.,
        })
        .collect();
    if out.is_empty() && used > 0 {
        out.push(Segment {
            label: "занято".into(),
            tokens: used,
            color: PALETTE[0],
            percent: 0.,
        });
    }
    if let Some(max) = max.filter(|max| *max > 0) {
        let buffer = trigger
            .filter(|trigger| *trigger < max)
            .map(|trigger| max - trigger)
            .unwrap_or(0)
            .min(max.saturating_sub(used));
        let free = max.saturating_sub(used).saturating_sub(buffer);
        for (label, tokens, color) in [
            ("свободно", free, FREE_COLOR),
            ("резерв автокомпакта", buffer, BUFFER_COLOR),
        ] {
            if tokens > 0 {
                out.push(Segment {
                    label: label.into(),
                    tokens,
                    color,
                    percent: 0.,
                });
            }
        }
    }
    let basis = max
        .filter(|max| *max > 0)
        .map(u64::from)
        .unwrap_or_else(|| out.iter().map(|s| u64::from(s.tokens)).sum::<u64>().max(1));
    for segment in &mut out {
        segment.percent = f64::from(segment.tokens) / basis as f64 * 100.;
    }
    out
}

/// Largest-remainder split of the map cells; a non-empty segment keeps at
/// least one cell so thin categories stay visible.
pub fn allocate_cells(tokens: &[u32], cells_total: usize) -> Vec<usize> {
    let total: u64 = tokens.iter().copied().map(u64::from).sum();
    if total == 0 || cells_total == 0 {
        return vec![0; tokens.len()];
    }
    let mut cells = Vec::with_capacity(tokens.len());
    let mut remainders = Vec::with_capacity(tokens.len());
    let mut allocated = 0;
    for (index, item) in tokens.iter().enumerate() {
        let exact = u64::from(*item) as f64 * cells_total as f64 / total as f64;
        cells.push(exact.floor() as usize);
        allocated += exact.floor() as usize;
        remainders.push((index, exact - exact.floor()));
    }
    remainders.sort_by(|left, right| right.1.total_cmp(&left.1));
    for (index, _) in remainders
        .into_iter()
        .take(cells_total.saturating_sub(allocated))
    {
        cells[index] += 1;
    }
    for index in 0..tokens.len() {
        if tokens[index] > 0
            && cells[index] == 0
            && let Some(largest) = (0..cells.len())
                .filter(|other| cells[*other] > 1)
                .max_by_key(|other| cells[*other])
        {
            cells[largest] -= 1;
            cells[index] += 1;
        }
    }
    cells
}

fn category_color(name: &str, index: usize) -> u32 {
    match name {
        "instructions" => 0x6b9eff,
        "messages" => 0x5cc784,
        "context" => 0xc08cf0,
        "tool_calls" => 0xe0975c,
        "tool_results" => 0xd8a21e,
        "tool_schemas" => 0x5cc0d8,
        "files" => 0x7fd4b2,
        "patches" => 0xef8fb7,
        _ => PALETTE[index % PALETTE.len()],
    }
}

fn category_label(name: &str) -> String {
    match name {
        "messages" => "messages/history".into(),
        "context" => "ephemeral context".into(),
        other => other.replace('_', " "),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheStatus {
    Unavailable,
    Cold,
    Warming,
    Hot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cache {
    pub status: CacheStatus,
    pub input: Option<u32>,
    pub cached: Option<u32>,
    pub written: Option<u32>,
    pub hit_percent: Option<u32>,
}

impl CacheStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "n/a",
            Self::Cold => "cold",
            Self::Warming => "warming",
            Self::Hot => "hot",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Self::Unavailable => "нет данных провайдера",
            Self::Cold => "кэш не читался",
            Self::Warming => "кэш прогревается",
            Self::Hot => "кэш активно читается",
        }
    }
}

pub fn cache(usage: Option<&AppContextUsageSnapshot>) -> Cache {
    let actual = usage.and_then(|usage| usage.actual.as_ref());
    let input = actual.map(|actual| actual.input_tokens);
    let cached = actual.and_then(|actual| actual.cached_input_tokens);
    let written = actual.and_then(|actual| actual.cache_creation_input_tokens);
    let hit_percent = match (input, cached) {
        (Some(input), Some(cached)) if input > 0 => Some(
            ((f64::from(cached) / f64::from(input)) * 100.)
                .round()
                .min(100.) as u32,
        ),
        _ => None,
    };
    let status = match input {
        None | Some(0) => CacheStatus::Unavailable,
        Some(_) => {
            let (read, write) = (cached.unwrap_or(0), written.unwrap_or(0));
            if read == 0 && write == 0 {
                CacheStatus::Cold
            } else if hit_percent.is_some_and(|hit| hit >= 50) {
                CacheStatus::Hot
            } else {
                CacheStatus::Warming
            }
        }
    };
    Cache {
        status,
        input,
        cached,
        written,
        hit_percent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn category(name: &str, tokens: u32) -> AppContextUsageCategory {
        serde_json::from_value(
            serde_json::json!({"name": name, "tokens": tokens, "source": "estimated"}),
        )
        .expect("category")
    }

    #[test]
    fn segments_scale_content_and_reserve_the_tail() {
        let categories = [
            category("instructions", 100),
            category("messages", 300),
            category("provider_cache_read", 999),
        ];
        let segments = segments(&categories, 1000, Some(10_000), Some(8_000));
        let labels: Vec<_> = segments.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "instructions",
                "messages/history",
                "свободно",
                "резерв автокомпакта"
            ]
        );
        assert_eq!(segments[0].tokens, 250);
        assert_eq!(segments[3].tokens, 2000);
        assert_eq!(segments[2].tokens, 7000);
    }

    #[test]
    fn cells_keep_thin_segments_visible() {
        let cells = allocate_cells(&[1, 10_000, 0], MAP_CELLS);
        assert_eq!(cells.iter().sum::<usize>(), MAP_CELLS);
        assert_eq!(cells[0], 1);
        assert_eq!(cells[2], 0);
    }
}
