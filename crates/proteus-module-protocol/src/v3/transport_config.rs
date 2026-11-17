//! One host-owned frame budget shared with a worker before initialization.
use anyhow::{Context, Result, ensure};
use proteus_process_host::{DEFAULT_MAX_FRAME_BYTES, ReceiveLimits};
use serde::{Deserialize, Serialize};

use super::ComponentBrokerOptions;

pub const COMPONENT_MAX_FRAME_BYTES_ENV: &str = "PROTEUS_MODULE_MAX_FRAME_BYTES";
pub const MAX_COMPONENT_FRAME_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ComponentTransportConfig {
    pub max_frame_bytes: usize,
}

impl Default for ComponentTransportConfig {
    fn default() -> Self {
        Self {
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
        }
    }
}

impl ComponentTransportConfig {
    pub fn validate(self) -> Result<()> {
        ensure!(
            (1..=MAX_COMPONENT_FRAME_BYTES).contains(&self.max_frame_bytes),
            "component transport max_frame_bytes must be between 1 and {MAX_COMPONENT_FRAME_BYTES}"
        );
        Ok(())
    }

    /// Rust workers read the launcher's explicit value before initialize. An
    /// absent value keeps standalone workers at the normal 8 MiB default.
    pub fn from_environment() -> Result<Self> {
        let value = match std::env::var(COMPONENT_MAX_FRAME_BYTES_ENV) {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => return Err(error).context("invalid component frame-budget environment"),
        };
        Self::from_environment_value(value.as_deref())
    }

    pub fn from_environment_value(value: Option<&str>) -> Result<Self> {
        let config = match value {
            None => Self::default(),
            Some(value) => Self {
                max_frame_bytes: value.parse().with_context(|| {
                    format!("{COMPONENT_MAX_FRAME_BYTES_ENV} must be a positive integer")
                })?,
            },
        };
        config.validate()?;
        Ok(config)
    }

    /// Raising one component's frame bound also raises its finite aggregate
    /// queues enough for two maximum-sized frames. Other components are unchanged.
    pub fn apply(self, mut options: ComponentBrokerOptions) -> Result<ComponentBrokerOptions> {
        self.validate()?;
        let aggregate = self
            .max_frame_bytes
            .checked_mul(2)
            .context("component transport byte budget overflow")?;
        options.max_outbound_frame_bytes = self.max_frame_bytes;
        options.max_inbound_frame_bytes = self.max_frame_bytes;
        options.data_writer_byte_capacity = options.data_writer_byte_capacity.max(aggregate);
        options.receive_limits = ReceiveLimits::new(
            options.receive_limits.max_buffered_frames(),
            options.receive_limits.max_buffered_bytes().max(aggregate),
        );
        options.validate()?;
        Ok(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_defaults_stay_identical_and_an_override_is_component_local() {
        let defaults = ComponentBrokerOptions::default();
        assert_eq!(
            ComponentTransportConfig::default().apply(defaults).unwrap(),
            defaults
        );
        let raised = ComponentTransportConfig {
            max_frame_bytes: MAX_COMPONENT_FRAME_BYTES,
        }
        .apply(defaults)
        .unwrap();
        assert_eq!(raised.max_outbound_frame_bytes, 128 * 1024 * 1024);
        assert_eq!(
            raised.receive_limits.max_buffered_bytes(),
            256 * 1024 * 1024
        );
        assert_eq!(raised.data_writer_byte_capacity, 256 * 1024 * 1024);
        assert_eq!(raised.notification_limits, defaults.notification_limits);
        assert_eq!(ComponentBrokerOptions::default(), defaults);
    }

    #[test]
    fn launch_environment_is_strictly_positive_and_bounded() {
        assert_eq!(
            ComponentTransportConfig::from_environment_value(None).unwrap(),
            ComponentTransportConfig::default()
        );
        for invalid in [
            "",
            "0",
            "-1",
            "1.5",
            " 1024",
            "134217729",
            "18446744073709551616",
        ] {
            assert!(
                ComponentTransportConfig::from_environment_value(Some(invalid)).is_err(),
                "{invalid}"
            );
        }
        assert_eq!(
            ComponentTransportConfig::from_environment_value(Some("134217728"))
                .unwrap()
                .max_frame_bytes,
            MAX_COMPONENT_FRAME_BYTES
        );
    }
}
