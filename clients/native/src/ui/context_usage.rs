use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppContextUsageSnapshot;
use proteus_contracts::domain::TokenUsageSnapshot;

/// Context window fill: the provider measurement when known, otherwise the
/// local estimate. Without a known window size there is nothing to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ContextUsage {
    pub used: u32,
    pub max: u32,
    pub trigger: Option<u32>,
}

impl ContextUsage {
    fn new(used: u32, max: Option<u32>, trigger: Option<u32>) -> Option<Self> {
        let max = max.filter(|max| *max > 0)?;
        Some(Self { used, max, trigger })
    }

    pub fn from_event(usage: &TokenUsageSnapshot) -> Option<Self> {
        let used = usage
            .actual
            .as_ref()
            .map_or(usage.estimated_input_tokens, |actual| actual.input_tokens);
        Self::new(
            used,
            usage.max_input_tokens,
            usage.compaction_trigger_tokens,
        )
    }

    pub fn from_snapshot(usage: &AppContextUsageSnapshot) -> Option<Self> {
        let used = usage
            .actual
            .as_ref()
            .map_or(usage.estimated_input_tokens, |actual| actual.input_tokens);
        Self::new(
            used,
            usage.max_input_tokens,
            usage.compaction_trigger_tokens,
        )
    }

    pub fn percent(self) -> f32 {
        (self.used as f32 / self.max as f32 * 100.).min(100.)
    }

    /// Ring and bar color: muted until the compaction trigger gets close.
    pub fn color(self, theme: &Theme) -> Hsla {
        match self.trigger {
            Some(trigger) if self.used >= trigger => theme.danger,
            Some(trigger) if self.used as f32 >= trigger as f32 * 0.8 => theme.warning,
            _ => theme.muted_foreground,
        }
    }
}

pub(super) fn tokens(value: u32) -> String {
    match value {
        0..1_000 => value.to_string(),
        1_000..1_000_000 => format!("{:.0}k", value as f32 / 1_000.),
        _ => format!("{:.1}M", value as f32 / 1_000_000.),
    }
}

#[cfg(test)]
mod tests {
    use super::ContextUsage;

    #[test]
    fn usage_needs_a_known_window() {
        let usage = ContextUsage::new(14_021, Some(200_000), Some(180_000)).unwrap();
        assert_eq!(usage.percent().round(), 7.);
        assert_eq!(super::tokens(usage.used), "14k");
        assert_eq!(ContextUsage::new(10, Some(0), None), None);
        assert_eq!(ContextUsage::new(10, None, None), None);
    }
}
