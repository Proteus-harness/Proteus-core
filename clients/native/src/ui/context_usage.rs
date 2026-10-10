use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppContextUsageSnapshot;
use proteus_contracts::domain::TokenUsageSnapshot;

use super::Workspace;

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

    fn describe(self) -> String {
        let mut text = format!(
            "Контекст: {} из {} ({:.0}%)",
            tokens(self.used),
            tokens(self.max),
            self.percent()
        );
        if let Some(trigger) = self.trigger {
            text.push_str(&format!(" · автокомпакт при {}", tokens(trigger)));
        }
        text
    }
}

pub(super) fn tokens(value: u32) -> String {
    match value {
        0..1_000 => value.to_string(),
        1_000..1_000_000 => format!("{:.0}k", value as f32 / 1_000.),
        _ => format!("{:.1}M", value as f32 / 1_000_000.),
    }
}

impl Workspace {
    pub(super) fn render_context_usage(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let usage = self.session.as_ref()?.context?;
        let color = usage.color(cx.theme());
        let description = SharedString::from(usage.describe());
        Some(
            div()
                .id("context-usage")
                .p_1()
                .child(
                    ProgressCircle::new("context-ring")
                        .value(usage.percent())
                        .color(color)
                        .size_4(),
                )
                .tooltip(move |window, cx| Tooltip::new(description.clone()).build(window, cx)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::ContextUsage;

    #[test]
    fn describes_usage_with_compaction_trigger() {
        let usage = ContextUsage::new(14_021, Some(200_000), Some(180_000)).unwrap();
        assert_eq!(
            usage.describe(),
            "Контекст: 14k из 200k (7%) · автокомпакт при 180k"
        );
        assert_eq!(ContextUsage::new(10, Some(0), None), None);
        assert_eq!(ContextUsage::new(10, None, None), None);
    }
}
