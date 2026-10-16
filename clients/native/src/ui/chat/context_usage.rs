use proteus_contracts::app_protocol::AppContextUsageSnapshot;
use proteus_contracts::domain::TokenUsageSnapshot;

/// Context window fill: the provider measurement when known, otherwise the
/// local estimate. Without a known window size there is nothing to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) struct ContextUsage {
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
}

#[cfg(test)]
mod tests {
    use super::ContextUsage;

    #[test]
    fn usage_needs_a_known_window() {
        let usage = ContextUsage::new(14_021, Some(200_000), Some(180_000)).unwrap();
        assert_eq!(usage.used, 14_021);
        assert_eq!(usage.max, 200_000);
        assert_eq!(ContextUsage::new(10, Some(0), None), None);
        assert_eq!(ContextUsage::new(10, None, None), None);
    }
}
