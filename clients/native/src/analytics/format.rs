//! Number and time formatting shared by the analytics pages.

/// Groups thousands with a narrow space: `12 345`.
pub fn tokens(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(digit);
    }
    out
}

/// US dollars with two to six fraction digits, like the web report.
pub fn money(value: f64) -> String {
    let mut text = format!("{value:.6}");
    while text.ends_with('0') && text.len() > text.find('.').unwrap_or(0) + 3 {
        text.pop();
    }
    format!("${text}")
}

pub fn seconds(start_ms: i64, end_ms: Option<i64>) -> String {
    match end_ms {
        Some(end) => format!("{:.1} с", (end - start_ms).max(0) as f64 / 1000.).replace('.', ","),
        None => "—".into(),
    }
}

/// Local wall-clock time of a journal timestamp.
pub fn clock(ms: i64) -> String {
    use chrono::TimeZone as _;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|time| time.format("%d.%m %H:%M:%S").to_string())
        .unwrap_or_else(|| "—".into())
}

pub fn short_id(id: impl ToString) -> String {
    id.to_string().chars().take(8).collect()
}

pub fn pretty(value: &impl serde::Serialize) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers() {
        assert_eq!(tokens(1234567), "1\u{202f}234\u{202f}567");
        assert_eq!(tokens(999), "999");
        assert_eq!(money(0.5), "$0.50");
        assert_eq!(money(0.001234), "$0.001234");
        assert_eq!(seconds(0, Some(1500)), "1,5 с");
    }
}
