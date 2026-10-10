use std::time::{SystemTime, UNIX_EPOCH};

use proteus_contracts::app_protocol::AppSessionSummary;

pub fn session_title(session: &AppSessionSummary) -> String {
    session
        .preview
        .as_deref()
        .map(str::trim)
        .filter(|preview| !preview.is_empty())
        .map(|preview| clip(preview, 80))
        .unwrap_or_else(|| "Новая сессия".to_owned())
}

pub fn relative_time(updated_at_ms: Option<u64>) -> String {
    let Some(updated) = updated_at_ms else {
        return String::new();
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(updated);
    let seconds = now.saturating_sub(updated) / 1000;
    match seconds {
        0..60 => "сейчас".to_owned(),
        60..3600 => format!("{} мин", seconds / 60),
        3600..86_400 => format!("{} ч", seconds / 3600),
        _ => format!("{} д", seconds / 86_400),
    }
}

/// First line of `text`, cut to `max_chars`.
pub fn clip(text: &str, max_chars: usize) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let line = line.trim();
    let multiline = text.trim().lines().nth(1).is_some();
    match line.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None if multiline => format!("{line} …"),
        None => line.to_owned(),
    }
}

/// Leading part of a long tool result, kept short enough for the thread.
pub fn clip_block(text: &str, max_lines: usize, max_chars: usize) -> String {
    let mut out = String::new();
    let mut lines = 0;
    for line in text.lines() {
        if lines == max_lines || out.len() + line.len() > max_chars {
            let rest = text.lines().count() - lines;
            out.push_str(&format!("… ещё строк: {rest}"));
            return out;
        }
        out.push_str(line);
        out.push('\n');
        lines += 1;
    }
    out.truncate(out.trim_end().len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_marks_cut_and_hidden_lines() {
        assert_eq!(clip("hello", 10), "hello");
        assert_eq!(clip("héllo world", 5), "héllo…");
        assert_eq!(clip("\nfirst\nsecond", 10), "first …");
    }

    #[test]
    fn clip_block_counts_hidden_lines() {
        assert_eq!(clip_block("a\nb\nc", 2, 100), "a\nb\n… ещё строк: 1");
        assert_eq!(clip_block("a\nb\n", 5, 100), "a\nb");
    }
}
