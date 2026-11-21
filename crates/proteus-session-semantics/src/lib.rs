//! Backend-independent session validation shared by host and storage workers.
//! This crate owns canonical folds, not files, transports or provider policy.
pub use proteus_contracts::{contracts, domain, model_standard};
pub mod journal;
pub mod model_context;
pub use journal::history_message_sha256;
pub use journal::redaction::{
    contains_redacted_sensitive_value, redact_sensitive_values, redacted_history,
};
pub use journal::{HistoryCapture, JournalProjection, JournalValidationState, UsageProjection};
pub use model_context::{ModelContextState, projection::ModelContextProjection};
