//! Source ownership for named behavior overlays, independent of resolved model settings.
use super::*;
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct BehaviorSourceMetadata {
    pub writable: bool,
}

impl BehaviorSourceMetadata {
    pub(super) fn capture(local: Option<&Value>, config: &AppConfig, file: bool) -> Self {
        let writable = file
            && local.is_some_and(|source| {
                source.get("include").is_none()
                    && serde_json::from_value::<
                        BTreeMap<String, crate::domain::BehaviorModeDefinition>,
                    >(
                        source
                            .get("modes")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({})),
                    )
                    .is_ok_and(|modes| modes == config.modes)
            });
        Self { writable }
    }
}

impl AppConfig {
    pub(crate) fn validate_behavior_modes(&self) -> Result<()> {
        for (id, definition) in &self.modes {
            anyhow::ensure!(
                !id.trim().is_empty() && id != "_default",
                "behavior mode ID must be nonempty and must not be _default"
            );
            if let Some(title) = &definition.title {
                anyhow::ensure!(
                    !title.trim().is_empty(),
                    "behavior mode title must not be empty"
                );
            }
            for (kind, list) in [("tools", &definition.tools), ("hooks", &definition.hooks)] {
                if let Some(list) = list {
                    let mut seen = std::collections::BTreeSet::new();
                    for value in list {
                        anyhow::ensure!(
                            !value.trim().is_empty() && seen.insert(value),
                            "behavior mode {id:?} has an empty or duplicate {kind} entry"
                        );
                    }
                }
            }
        }
        Ok(())
    }
}
