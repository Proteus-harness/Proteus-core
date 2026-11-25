//! Helpers for process-peer terminal results.

use crate::contracts::AgentLifecycleStatus;

/// Canonical snake_case label для `Event::SubagentFinished`.
pub(super) fn status_label(status: AgentLifecycleStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{status:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::agent_control::retain_summary;

    #[test]
    fn summary_truncation_respects_char_boundaries() {
        let truncated = retain_summary("й".repeat(4), 5);
        assert_eq!(truncated, "йй");
        assert!(truncated.len() <= 5);
        assert!(truncated.is_char_boundary(truncated.len()));
        assert_eq!(retain_summary("abc".to_owned(), 5), "abc");
    }

    #[test]
    fn summary_releases_the_original_capacity() {
        let text = "я".repeat(1 << 20);
        let summary = retain_summary(text, 8191);
        assert_eq!(summary, "я".repeat(4095));
        assert_eq!(summary.capacity(), summary.len());

        let mut already_short = String::with_capacity(1 << 20);
        already_short.push_str("short");
        let summary = retain_summary(already_short, 8192);
        assert_eq!(summary, "short");
        assert_eq!(summary.capacity(), summary.len());
    }

    #[test]
    fn status_label_is_snake_case() {
        assert_eq!(status_label(AgentLifecycleStatus::Completed), "completed");
        assert_eq!(status_label(AgentLifecycleStatus::TimedOut), "timed_out");
        assert_eq!(status_label(AgentLifecycleStatus::Cancelled), "cancelled");
    }
}
