//! Named behavior overlays. These select existing capabilities; they grant no authority.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BehaviorModeDefinition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Additional inline instructions, appended to the profile instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// A complete callable-tool allowlist, including automatically registered
    /// MCP, agent-control and provider-hosted tools. None inherits availability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// An ordered replacement for modules.hooks; None inherits the baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<super::PermissionMode>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permission_values_round_trip_without_upgrading_legacy_auto() {
        use super::super::PermissionMode;
        for mode in PermissionMode::SELECTABLE
            .into_iter()
            .chain([PermissionMode::Auto])
        {
            let value = serde_json::to_value(mode).unwrap();
            assert_eq!(value, mode.as_str());
            assert_eq!(
                serde_json::from_value::<PermissionMode>(value).unwrap(),
                mode
            );
        }
        assert!(!PermissionMode::SELECTABLE.contains(&PermissionMode::Auto));
        assert_eq!(
            serde_json::from_str::<PermissionMode>("\"auto\"").unwrap(),
            PermissionMode::Auto
        );
    }

    #[test]
    fn definitions_distinguish_inheritance_from_empty_lists_and_reject_other_settings() {
        let inherited: BehaviorModeDefinition = serde_json::from_str("{}").unwrap();
        let empty: BehaviorModeDefinition =
            serde_json::from_str(r#"{"tools":[],"hooks":[]}"#).unwrap();
        assert_eq!(inherited.tools, None);
        assert_eq!(empty.tools, Some(vec![]));
        assert_eq!(empty.hooks, Some(vec![]));
        assert_eq!(
            serde_json::to_value(inherited).unwrap(),
            serde_json::json!({})
        );
        assert!(serde_json::from_str::<BehaviorModeDefinition>(r#"{"model":"cheap"}"#).is_err());
        let full: BehaviorModeDefinition =
            serde_json::from_str(r#"{"permission_mode":"full_access"}"#).unwrap();
        assert_eq!(
            full.permission_mode,
            Some(super::super::PermissionMode::FullAccess)
        );
        assert!(
            serde_json::from_str::<BehaviorModeDefinition>(r#"{"permission_mode":"unknown"}"#)
                .is_err()
        );
    }
}
