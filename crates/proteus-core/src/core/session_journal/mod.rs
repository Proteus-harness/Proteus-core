//! Canonical execution facts and host recorders; physical storage is external.
mod recorder;
pub use proteus_contracts::storage::journal::*;
pub use proteus_session_semantics::JournalProjection;
pub(crate) use proteus_session_semantics::{
    JournalValidationState, history_message_sha256, redacted_history,
};
pub use recorder::{SessionExecutionRecorder, SessionToolExecutionRecorder};
pub(crate) mod history_capture {
    pub(crate) use proteus_session_semantics::HistoryCapture;
}
pub(crate) mod hooks {
    pub(crate) use proteus_session_semantics::journal::hooks::*;
}
pub(crate) use crate::core::session_store::remote::snapshot::{
    RemoteJournalCursor as JournalReadCursor, RemoteJournalResumeState as JournalResumeState,
    RemoteJournalSnapshot as JournalReadSnapshot, RemoteValidatedRead as ValidatedJournalRead,
};
pub(crate) use hooks::apply_recorded_hook_response;
// The default backend's existing archive constants remain descriptive, never a reader.
pub const JOURNAL_FILE: &str = "journal.jsonl";
pub const DEFAULT_BLOB_THRESHOLD_BYTES: usize = 256 * 1024;
#[cfg(test)]
mod context_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod hooks_tests;

pub(crate) use crate::core::session_store::remote::snapshot::is_display_kind;
