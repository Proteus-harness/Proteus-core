//! Pure canonical journal validation and projections. No files or backend I/O.
mod context;
pub mod history_capture;
pub mod hooks;
pub mod message_hash;
pub mod projection;
pub mod redaction;
pub mod usage;
pub mod types {
    pub use proteus_contracts::storage::journal::*;
}
pub use history_capture::HistoryCapture;
pub use message_hash::history_message_sha256;
pub use projection::{JournalProjection, JournalValidationState};
pub use types::*;
pub use usage::UsageProjection;
