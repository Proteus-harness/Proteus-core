use std::time::{SystemTime, UNIX_EPOCH};

use proteus_contracts::app_protocol::{AppSessionSummary, AppTranscriptTool};
use serde_json::Value;

const SUMMARY_KEYS: &[&str] = &[
    "command", "cmd", "path", "file_path", "pattern", "query", "url", "description",
];

pub fn session_title(session: &AppSessionSummary) -> String {
    session
        .preview
        .as_deref()
        .map(str::trim)
        .filter(|preview| !preview.is_empty())
        .map(|preview| clip(preview, 80))
        .unwrap_or_else(|| "New session".to_owned())
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
        0..60 => "now".to_owned(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// One-line description of what a tool call operates on.
pub fn tool_summary(tool: &AppTranscriptTool) -> String {
    let args = tool.effective_args.as_ref().unwrap_or(&tool.args);
    let picked = args.as_object().and_then(|object| {
        SUMMARY_KEYS
            .iter()
            .find_map(|key| object.get(*key))
            .map(|value| match value {
                Value::String(text) => text.clone(),
                Value::Array(items) => items
                    .iter()
                    .map(|item| item.as_str().map_or_else(|| item.to_string(), str::to_owned))
                    .collect::<Vec<_>>()
                    .join(" "),
                other => other.to_string(),
            })
    });
    let text = match picked {
        Some(text) => text,
        None if args.is_null() => String::new(),
        None => args.to_string(),
    };
    clip(&text, 160)
}

/// First line of `text`, cut to `max_chars`.
pub fn clip(text: &str, max_chars: usize) -> String {
    let line = text.lines().find(|line| !line.trim().is_empty()).unwrap_or("");
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
            out.push_str(&format!("… {rest} more lines"));
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
        assert_eq!(clip_block("a\nb\nc", 2, 100), "a\nb\n… 1 more lines");
        assert_eq!(clip_block("a\nb\n", 5, 100), "a\nb");
    }
}
