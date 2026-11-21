//! Advisory provider-owned sizing for opaque input. It grants no capabilities.
use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{CanonicalMessage, CanonicalPart, ContentPart};
use crate::domain::{ModelRef, PartId};

pub const INPUT_TOKEN_ESTIMATES_METADATA_KEY: &str = "model/input_token_estimates";
pub const CONTEXT_TOKEN_ESTIMATE_SOURCE_METADATA_KEY: &str = "context/token_estimate_source";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelInputEstimateRequest {
    pub model: ModelRef,
    pub parts: Vec<CanonicalPart>,
}

impl ModelInputEstimateRequest {
    pub fn validate(&self) -> Result<(), String> {
        self.validation_context().map(drop)
    }

    /// Snapshot only opaque part identities before moving the request to a provider.
    pub fn validation_context(&self) -> Result<ModelInputEstimateValidation, String> {
        let mut ids = HashSet::new();
        for part in &self.parts {
            if !ids.insert(part.part_id) || !matches!(part.payload, ContentPart::Opaque { .. }) {
                return Err("input estimate requires unique canonical opaque parts".into());
            }
        }
        Ok(ModelInputEstimateValidation { ids })
    }
}

/// One validated request, one optional partial response. Never retains opaque payloads.
pub struct ModelInputEstimateValidation {
    ids: HashSet<PartId>,
}

impl ModelInputEstimateValidation {
    pub fn validate_response(mut self, estimate: &ModelInputEstimate) -> Result<(), String> {
        for part in &estimate.parts {
            if !self.ids.remove(&part.part_id) {
                return Err("input estimate contains an unknown or duplicate part id".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct ModelInputEstimate {
    pub parts: Vec<ModelPartTokenEstimate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelPartTokenEstimate {
    pub part_id: PartId,
    pub tokens: u32,
}

impl ModelInputEstimate {
    pub fn validate(&self, request: &ModelInputEstimateRequest) -> Result<(), String> {
        request.validation_context()?.validate_response(self)
    }

    pub fn tokens_for(&self, part: &CanonicalPart) -> Option<u32> {
        if !matches!(part.payload, ContentPart::Opaque { .. }) {
            return None;
        }
        self.parts
            .iter()
            .find(|estimate| estimate.part_id == part.part_id)
            .map(|estimate| estimate.tokens)
    }

    /// Only matching opaque identities survive; a hint never changes material.
    pub fn retain_for(&mut self, messages: &[CanonicalMessage]) {
        let allowed: HashSet<_> = messages
            .iter()
            .flat_map(|message| &message.parts)
            .filter(|part| matches!(part.payload, ContentPart::Opaque { .. }))
            .map(|part| part.part_id)
            .collect();
        self.parts
            .retain(|estimate| allowed.contains(&estimate.part_id));
    }

    pub fn from_metadata(metadata: &Value) -> Option<Self> {
        let value = metadata.get(INPUT_TOKEN_ESTIMATES_METADATA_KEY)?;
        let estimate = Self::deserialize(value).ok()?;
        let mut seen = HashSet::new();
        estimate
            .parts
            .iter()
            .all(|part| seen.insert(part.part_id))
            .then_some(estimate)
    }

    /// Preserve unrelated metadata, including non-object user-owned values.
    pub fn write_metadata(&self, metadata: &mut Value) -> bool {
        let mut seen = HashSet::new();
        if !self.parts.iter().all(|part| seen.insert(part.part_id)) {
            return false;
        }
        if metadata.is_null() {
            *metadata = serde_json::json!({});
        }
        let Some(object) = metadata.as_object_mut() else {
            return false;
        };
        object.insert(
            INPUT_TOKEN_ESTIMATES_METADATA_KEY.into(),
            serde_json::to_value(self).expect("canonical input estimates"),
        );
        true
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextTokenEstimateSource {
    Estimated,
    Observed,
}

impl ContextTokenEstimateSource {
    pub fn from_metadata(metadata: &Value) -> Option<Self> {
        serde_json::from_value(
            metadata
                .get(CONTEXT_TOKEN_ESTIMATE_SOURCE_METADATA_KEY)?
                .clone(),
        )
        .ok()
    }

    pub fn write_metadata(self, metadata: &mut Value) -> bool {
        if metadata.is_null() {
            *metadata = serde_json::json!({});
        }
        let Some(object) = metadata.as_object_mut() else {
            return false;
        };
        object.insert(
            CONTEXT_TOKEN_ESTIMATE_SOURCE_METADATA_KEY.into(),
            serde_json::to_value(self).expect("canonical estimate source"),
        );
        true
    }
}

#[cfg(test)]
mod tests;
