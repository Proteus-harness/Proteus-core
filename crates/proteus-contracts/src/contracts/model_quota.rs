//! Provider-neutral account quota discovery, independent of inference and UI.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelQuotaSnapshot {
    /// Time the provider response was obtained, including when served from cache.
    pub observed_at: u64,
    pub plan: Option<String>,
    pub buckets: Vec<ModelQuotaBucket>,
    pub credits: Option<ModelQuotaCredits>,
    /// Provider notices about partially unavailable account metadata.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelQuotaBucket {
    pub id: String,
    pub name: Option<String>,
    pub allowed: Option<bool>,
    pub limit_reached: Option<bool>,
    pub windows: Vec<ModelQuotaWindow>,
    /// Additional account resources whose applicability to the active model is
    /// unknown. They must not determine its remaining allowance.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub informational: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelQuotaWindow {
    pub id: String,
    /// May exceed 100 when the provider reports overage; never inferred from tokens.
    pub used_percent: f64,
    pub duration_seconds: Option<u64>,
    /// Unix timestamp in seconds; null means the provider did not report it.
    pub resets_at: Option<u64>,
    /// Absolute usage in a provider-reported unit, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<ModelQuotaAmount>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelQuotaAmount {
    pub used: f64,
    pub limit: f64,
    /// For example, USD. No currency conversion or token-based estimation.
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelQuotaCredits {
    pub available: bool,
    pub unlimited: bool,
    /// Provider-denominated decimal amount. No currency conversion or cost estimate.
    pub balance: Option<String>,
}

impl ModelQuotaSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        if self.observed_at == 0 {
            return Err("quota observed_at must be a positive Unix timestamp".into());
        }
        if self
            .plan
            .as_ref()
            .is_some_and(|plan| plan.trim().is_empty())
        {
            return Err("quota plan must not be empty".into());
        }
        if self.notices.iter().any(|notice| notice.trim().is_empty()) {
            return Err("quota notice must not be empty".into());
        }
        let mut buckets = std::collections::HashSet::new();
        for bucket in &self.buckets {
            if bucket.id.trim().is_empty() || !buckets.insert(&bucket.id) {
                return Err("quota contains an empty or duplicate bucket id".into());
            }
            let mut windows = std::collections::HashSet::new();
            for window in &bucket.windows {
                if window.id.trim().is_empty() || !windows.insert(&window.id) {
                    return Err("quota contains an empty or duplicate window id".into());
                }
                if !window.used_percent.is_finite() || window.used_percent < 0.0 {
                    return Err("quota used_percent must be finite and non-negative".into());
                }
                if window.duration_seconds == Some(0) {
                    return Err("quota window duration must be positive or null".into());
                }
                if let Some(amount) = &window.amount {
                    if !amount.used.is_finite()
                        || amount.used < 0.0
                        || !amount.limit.is_finite()
                        || amount.limit < 0.0
                        || amount.unit.trim().is_empty()
                    {
                        return Err("quota amount must contain finite non-negative usage and limit, and a non-empty unit".into());
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot() -> ModelQuotaSnapshot {
        serde_json::from_value(json!({
            "observed_at": 1_900_000_000,
            "plan": null,
            "credits": null,
            "buckets": [{
                "id": "budget", "name": "Shared budget",
                "allowed": null, "limit_reached": null,
                "informational": true,
                "windows": [{
                    "id": "budget", "used_percent": 125,
                    "duration_seconds": null, "resets_at": null,
                    "amount": {"used": 12.5, "limit": 10, "unit": "USD"}
                }]
            }],
            "notices": ["Additional account details unavailable"]
        }))
        .unwrap()
    }

    #[test]
    fn quota_retains_absolute_overage_and_informational_account_limits() {
        let quota = snapshot();
        quota.validate().unwrap();
        let encoded = serde_json::to_value(&quota).unwrap();
        assert_eq!(encoded["buckets"][0]["informational"], true);
        assert_eq!(encoded["buckets"][0]["windows"][0]["amount"]["used"], 12.5);
        assert_eq!(
            serde_json::from_value::<ModelQuotaSnapshot>(encoded).unwrap(),
            quota
        );
    }

    #[test]
    fn quota_missing_optional_account_details_stays_unspecified() {
        let mut stored = serde_json::to_value(snapshot()).unwrap();
        stored.as_object_mut().unwrap().remove("notices");
        let bucket = stored["buckets"][0].as_object_mut().unwrap();
        bucket.remove("informational");
        bucket["windows"][0]
            .as_object_mut()
            .unwrap()
            .remove("amount");
        let quota: ModelQuotaSnapshot = serde_json::from_value(stored.clone()).unwrap();
        quota.validate().unwrap();
        assert_eq!(serde_json::to_value(quota).unwrap(), stored);
    }

    #[test]
    fn quota_rejects_unusable_absolute_amounts_and_blank_notices() {
        for value in [f64::NAN, f64::INFINITY, -1.] {
            for used in [true, false] {
                let mut quota = snapshot();
                let amount = quota.buckets[0].windows[0].amount.as_mut().unwrap();
                if used {
                    amount.used = value;
                } else {
                    amount.limit = value;
                }
                assert!(quota.validate().is_err());
            }
        }
        let mut quota = snapshot();
        quota.buckets[0].windows[0].amount.as_mut().unwrap().unit = " ".into();
        assert!(quota.validate().is_err());
        let mut quota = snapshot();
        quota.notices = vec![" ".into()];
        assert!(quota.validate().is_err());
    }
}
