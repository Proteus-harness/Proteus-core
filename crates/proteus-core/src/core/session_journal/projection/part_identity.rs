//! Compact, process-local identities for the large, common canonical payloads.
//! Hash typed fields rather than JSON bytes: Value's Hash preserves its equality
//! rules (including signed zero), and Path's Hash preserves path equality.
use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use proteus_contracts::{
    domain::ToolContent,
    model_standard::{CanonicalPart, ContentPart},
};
use ring::digest::{Context, SHA256};

#[derive(Debug, Clone, PartialEq)]
pub(super) enum PartIdentity {
    Digest([u8; 32]),
    // Complex and future variants retain exact comparison until their complete
    // equality semantics have an explicit fingerprint implementation.
    Exact(Arc<CanonicalPart>),
}

impl PartIdentity {
    pub(super) fn matches(&self, part: &CanonicalPart) -> bool {
        match self {
            Self::Exact(previous) => previous.as_ref() == part,
            Self::Digest(_) => self == &Self::new(part),
        }
    }

    pub(super) fn new(part: &CanonicalPart) -> Self {
        let mut hash = DigestHasher(Context::new(&SHA256));
        std::mem::discriminant(&part.provenance).hash(&mut hash);
        std::mem::discriminant(&part.scope).hash(&mut hash);
        std::mem::discriminant(&part.payload).hash(&mut hash);
        match &part.payload {
            ContentPart::Text { text } | ContentPart::ReasoningSummary { text } => {
                text.hash(&mut hash)
            }
            ContentPart::Reasoning { text, signature } => (text, signature).hash(&mut hash),
            ContentPart::FileRef { path, content } => (path, content).hash(&mut hash),
            ContentPart::ToolCall { call } => {
                (&call.id, &call.name, &call.args, &call.raw_arguments).hash(&mut hash);
                std::mem::discriminant(&call.surface).hash(&mut hash);
            }
            ContentPart::ToolResult { result } => {
                (
                    &result.call_id,
                    result.ok,
                    &result.output,
                    &result.error,
                    &result.metadata,
                )
                    .hash(&mut hash);
                result.content.len().hash(&mut hash);
                for content in &result.content {
                    std::mem::discriminant(content).hash(&mut hash);
                    match content {
                        ToolContent::Text { text } => text.hash(&mut hash),
                        ToolContent::Json { value } => value.hash(&mut hash),
                        ToolContent::Image { mime_type, data }
                        | ToolContent::Binary { mime_type, data } => {
                            (mime_type, data).hash(&mut hash)
                        }
                        _ => return Self::Exact(Arc::new(part.clone())),
                    }
                }
            }
            _ => return Self::Exact(Arc::new(part.clone())),
        }
        let digest = hash.0.finish();
        Self::Digest(digest.as_ref().try_into().expect("SHA256 length"))
    }
}

struct DigestHasher(Context);
impl Hasher for DigestHasher {
    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
    fn finish(&self) -> u64 {
        panic!("use the complete SHA256 digest")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proteus_contracts::{
        domain::ToolResult,
        model_standard::{PartProvenance, PartScope},
    };
    use serde_json::json;

    #[test]
    fn fingerprints_preserve_json_equality_and_check_all_large_result_fields() {
        let mut part = CanonicalPart::new(
            PartProvenance::Tool,
            PartScope::Conversation,
            ContentPart::ToolResult {
                result: ToolResult::ok("call".into(), "x".repeat(1_048_576))
                    .with_metadata(json!({"zero": -0.0}))
                    .with_content(vec![ToolContent::Json {
                        value: json!({"a": 1}),
                    }]),
            },
        );
        let original = PartIdentity::new(&part);
        assert!(matches!(original, PartIdentity::Digest(_)));
        let ContentPart::ToolResult { result } = &mut part.payload else {
            unreachable!()
        };
        result.metadata = json!({"zero": 0.0});
        assert_eq!(original, PartIdentity::new(&part));
        let mut mutations = Vec::new();
        for index in 0..8 {
            let mut changed = part.clone();
            let ContentPart::ToolResult { result } = &mut changed.payload else {
                unreachable!()
            };
            match index {
                0 => result.call_id = "other".into(),
                1 => result.ok = false,
                2 => result.output.push('!'),
                3 => result.error = Some("error".into()),
                4 => result.content.push(ToolContent::Text {
                    text: "changed".into(),
                }),
                5 => result.metadata = json!({"changed": true}),
                6 => changed.scope = PartScope::Trace,
                _ => changed.provenance = PartProvenance::Runtime,
            }
            mutations.push(changed);
        }
        for changed in mutations {
            assert_ne!(original, PartIdentity::new(&changed));
        }
    }
}
